//! Every tool, called with hostile arguments (D2) and with paths that try to leave `--root` (D3).
//!
//! AGENTS.md §3 and §4 treat tool arguments as untrusted — they are the MCP attack surface, and an
//! agent can send any JSON at all. But `tools::check_args` only checks the *shape* of an argument
//! object: that it is an object, that it has no unknown keys, and that the required keys are
//! present and not null. **Types and ranges are not validated there**, so a string where an
//! integer belongs, `i64::MIN` where a page number belongs, or a 100,000-character path all reach
//! the handler. Each handler has to cope on its own, and nothing checked that they all did.
//!
//! These tests call every tool in the table with a zoo of hostile values and require two things:
//!
//! 1. **Nothing panics.** A panic in a tool is a crashed MCP server or CLI (AGENTS.md §4), so
//!    every call must come back as `Ok` or as a `ToolError` the agent can read.
//! 2. **Nothing escapes `--root`.** Every path argument is tried with `..` traversal, absolute
//!    paths, Windows drive and UNC paths, and a planted symbolic link. The call must fail, and
//!    afterwards nothing outside the root may have been created.
//!
//! The second is the mechanized form of the D3 audit. Reading the code says every handler resolves
//! its own paths and that no tool dispatches another internally; this keeps that true as tools are
//! added, which a written finding would not.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use pdfcraft_automation::{Automation, ToolError, tools};
use serde_json::{Value, json};

/// A fresh empty directory for one test.
fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pdfcraft-hostile-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("temp dir");
    d
}

/// Values chosen to break assumptions: wrong types, the ends of the numeric ranges, strings that
/// are empty, enormous, or full of separators and control characters, and containers that are
/// empty, mistyped or deeply nested.
fn zoo() -> Vec<Value> {
    let deep = {
        let mut v = json!(0);
        for _ in 0..64 {
            v = json!([v]);
        }
        v
    };
    vec![
        Value::Null,
        json!(true),
        json!(false),
        json!(0),
        json!(-1),
        json!(1),
        json!(i64::MAX),
        json!(i64::MIN),
        json!(u64::MAX),
        json!(0.5),
        json!(-0.0),
        json!(1e308),
        json!(-1e308),
        json!(""),
        json!(" "),
        json!("0"),
        json!("-1"),
        json!("null"),
        json!("\0"),
        json!("\u{202E}gnp.txe"), // right-to-left override: a name that displays reversed
        json!("💥🙈"),
        json!("%n%s%p"),
        json!("a".repeat(100_000)),
        json!("../".repeat(64)),
        json!([]),
        json!([null]),
        json!([i64::MIN, i64::MAX]),
        json!(["", "../.."]),
        json!({}),
        json!({ "unexpected": 1 }),
        deep,
    ]
}

/// Paths that must never resolve inside the root. Absolute ones point into the test's own temp
/// `base` (the root's parent), never at real system files or network shares, so a confinement
/// bug can only touch what the test made.
fn escapes(base: &std::path::Path, linked: bool) -> Vec<String> {
    let mut out: Vec<String> = vec![
        "../outside.pdf".into(),
        "../../outside.pdf".into(),
        "../".repeat(40) + "outside.pdf",
        "sub/../../outside.pdf".into(),
        "./././../outside.pdf".into(),
        base.join("outside.pdf").to_string_lossy().into_owned(),
        base.join("new-outside.pdf").to_string_lossy().into_owned(),
        base.join("sub").join("new-outside.pdf").to_string_lossy().into_owned(),
    ];
    // Through a planted symbolic link, only when one could actually be made: without it
    // `link-out/` is an ordinary folder inside the root, where writing is no escape.
    if linked {
        out.push("link-out/outside.pdf".into());
    }
    // Backslashes separate only on Windows; elsewhere this is an ordinary file name in the root.
    if cfg!(windows) {
        out.push(r"..\..\outside.pdf".into());
    }
    out
}

/// The properties of `def` whose values are paths, by name and whether they are a list.
fn path_props(def: &pdfcraft_automation::ToolDef) -> Vec<(String, bool)> {
    let Some(props) = def.input_schema["properties"].as_object() else { return Vec::new() };
    props
        .iter()
        .filter(|(name, spec)| {
            // `out` (an output file) and `image` (a picture to place) are paths when they are
            // strings; `image` is an image number elsewhere, which the integer check skips.
            let is_pathish = matches!(name.as_str(), "path" | "paths" | "folder" | "out_dir" | "file" | "out" | "image")
                || spec["description"].as_str().is_some_and(|d| d.starts_with("A file path"));
            // `file_page` is a page number inside a file, not a path.
            is_pathish && spec["type"] != "integer"
        })
        .map(|(name, spec)| (name.clone(), spec["type"] == "array"))
        .collect()
}

/// An arguments object giving every required property a plausible value, so a hostile value for
/// one property is not masked by a "missing argument" error for another.
///
/// `doc` gets a **real** open document when one is given. Without that, almost every tool fails at
/// `self.doc()` before it ever looks at a path, and a confinement test would pass vacuously — as
/// an earlier version of this file did, even with `resolve` disabled entirely.
fn plausible_with(def: &pdfcraft_automation::ToolDef, doc: Option<u64>) -> serde_json::Map<String, Value> {
    let mut obj = plausible(def);
    if let Some(id) = doc
        && def.input_schema["properties"].as_object().is_some_and(|p| p.contains_key("doc"))
    {
        obj.insert("doc".into(), json!(id));
    }
    obj
}

fn plausible(def: &pdfcraft_automation::ToolDef) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    let props = def.input_schema["properties"].as_object().cloned().unwrap_or_default();
    for req in def.input_schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let spec = props.get(req).cloned().unwrap_or(Value::Null);
        let value = match spec["type"].as_str() {
            Some("integer") => json!(1),
            Some("number") => json!(1.0),
            Some("boolean") => json!(false),
            Some("array") => json!([]),
            Some("object") => json!({}),
            _ => json!("in.pdf"),
        };
        obj.insert(req.to_string(), value);
    }
    obj
}

/// Run `call`, turning a panic into a message instead of unwinding out of the test.
fn guarded(a: &mut Automation, tool: &str, args: &Value) -> std::result::Result<(), String> {
    let result = catch_unwind(AssertUnwindSafe(|| a.call(tool, args)));
    match result {
        Ok(Ok(_)) => Ok(()),
        // Any ToolError is a good outcome: the agent is told what was wrong.
        Ok(Err(ToolError::UnknownTool(_) | ToolError::InvalidArgs(_) | ToolError::Failed(_))) => Ok(()),
        Err(panic) => {
            let what = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into());
            Err(format!("{tool} panicked on {args}: {what}"))
        }
    }
}

#[test]
fn no_tool_panics_on_hostile_arguments() {
    let root = tmp("args");
    let mut a = Automation::new().with_root(&root).expect("root");
    let values = zoo();
    let mut failures = Vec::new();
    let mut calls = 0usize;

    for def in tools() {
        // The whole arguments object, not just its properties.
        for bad in [json!(null), json!([]), json!("string"), json!(7), json!(true), json!({})] {
            calls += 1;
            if let Err(e) = guarded(&mut a, def.name, &bad) {
                failures.push(e);
            }
        }
        let props: Vec<String> = def.input_schema["properties"].as_object().map(|p| p.keys().cloned().collect()).unwrap_or_default();
        for prop in &props {
            for value in &values {
                let mut obj = plausible(&def);
                obj.insert(prop.clone(), value.clone());
                calls += 1;
                if let Err(e) = guarded(&mut a, def.name, &Value::Object(obj)) {
                    failures.push(e);
                }
            }
        }
    }

    eprintln!("hostile arguments: {calls} calls over {} tools, {} panics", tools().len(), failures.len());
    for f in failures.iter().take(20) {
        eprintln!("  {f}");
    }
    let _ = std::fs::remove_dir_all(&root);
    assert!(failures.is_empty(), "{} tool call(s) panicked; the first is: {}", failures.len(), failures.first().map_or("", String::as_str));
}

#[test]
fn no_tool_escapes_the_root() {
    let base = tmp("escape");
    let root = base.join("root");
    std::fs::create_dir_all(&root).expect("root");
    // What a traversal would reach, and the state we require to be unchanged afterwards.
    let outside = base.join("outside.pdf");
    std::fs::write(
        &outside,
        b"%PDF-1.7
% not yours
",
    )
    .expect("outside file");
    let before = listing(&base);

    // A link inside the root pointing out of it, where the system allows one to be made. Windows
    // only lets a standard user create one with Developer Mode on, so this can fail: when it does,
    // the link-out escape is skipped rather than tried against an ordinary folder inside the root.
    #[cfg(unix)]
    let link = std::os::unix::fs::symlink(&base, root.join("link-out"));
    #[cfg(windows)]
    let link = std::os::windows::fs::symlink_dir(&base, root.join("link-out"));
    let linked = link.map_err(|e| eprintln!("symbolic link escapes not checked: {e}")).is_ok();

    let mut a = Automation::new().with_root(&root).expect("root");

    // A real document, so tools reach their path arguments instead of failing on `doc` first.
    let created = a.call("doc_create", &json!({ "from": "blank", "pages": 1 })).expect("create a blank document");
    let doc = created
        .into_iter()
        .find_map(|c| match c {
            pdfcraft_automation::Content::Json(v) => v["doc"].as_u64(),
            pdfcraft_automation::Content::Png { .. } => None,
        })
        .expect("the new document has an id");

    let mut leaked = Vec::new();
    let mut tried = 0usize;
    let mut refused = 0usize;

    for def in tools() {
        for (prop, is_list) in path_props(&def) {
            for escape in escapes(&base, linked) {
                let mut obj = plausible_with(&def, Some(doc));
                obj.insert(prop.clone(), if is_list { json!([escape]) } else { json!(escape) });
                let args = Value::Object(obj);
                tried += 1;
                match catch_unwind(AssertUnwindSafe(|| a.call(def.name, &args))) {
                    // A path that leaves the root must be refused. Succeeding means the tool
                    // either used it or ignored an argument it declares — both are faults.
                    Ok(Ok(_)) => leaked.push(format!("{} accepted {prop}={escape:?}", def.name)),
                    Ok(Err(_)) => refused += 1,
                    Err(_) => leaked.push(format!("{} panicked on {prop}={escape:?}", def.name)),
                }
            }
        }
    }

    // Second net: nothing outside the root may have appeared, vanished or changed.
    let after = listing(&base);
    for (path, size) in &after {
        if path.starts_with(&root) {
            continue;
        }
        match before.iter().find(|(p, _)| p == path) {
            Some((_, was)) if was == size => {}
            Some(_) => leaked.push(format!("{} was modified outside the root", path.display())),
            None => leaked.push(format!("{} was created outside the root", path.display())),
        }
    }
    for (path, _) in &before {
        if !path.starts_with(&root) && !after.iter().any(|(p, _)| p == path) {
            leaked.push(format!("{} was deleted outside the root", path.display()));
        }
    }

    eprintln!("root confinement: {tried} escape attempts over {} tools, {refused} refused, {} leaks", tools().len(), leaked.len());
    for l in leaked.iter().take(20) {
        eprintln!("  {l}");
    }
    let _ = std::fs::remove_dir_all(&base);
    assert!(tried > 100, "only {tried} escape attempts: the path properties are not being found");
    assert!(leaked.is_empty(), "{} confinement failure(s); the first is: {}", leaked.len(), leaked.first().map_or("", String::as_str));
}

/// Every regular file under `dir` with its size, not following symbolic links.
fn listing(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            if meta.file_type().is_symlink() {
                continue; // do not walk through a link we planted
            }
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                out.push((path, meta.len()));
            }
        }
    }
    out.sort();
    out
}
