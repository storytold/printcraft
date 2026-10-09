//! The redaction tools: full reports, partial vs complete results, and saving redacted output.

use std::path::{Path, PathBuf};

use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

fn workdir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-redact-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn auto(dir: &Path) -> Automation {
    Automation::new().with_root(dir).unwrap().with_clock(|| 1_700_000_000)
}

fn ok(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args) {
        Ok(mut c) => match c.remove(0) {
            Content::Json(v) => v,
            other => panic!("{tool}: expected JSON, got {other:?}"),
        },
        Err(e) => panic!("{tool} {args}: {e}"),
    }
}

fn texts(a: &mut Automation, doc: u64) -> Vec<String> {
    ok(a, "text_extract", json!({ "doc": doc }))["pages"].as_array().unwrap().iter().map(|p| p["text"].as_str().unwrap().to_string()).collect()
}

const SECRET: &str = "Zebra-4711";

/// A document of three pages (about 60 lines fit on one) with the secret on pages 1 and 3.
fn open_memo(a: &mut Automation, dir: &Path) -> u64 {
    let mut memo: Vec<String> = (0..150).map(|i| format!("Line {i}")).collect();
    memo[1] = format!("The code is {SECRET}.");
    memo[140] = format!("Again {SECRET}.");
    std::fs::write(dir.join("memo.txt"), memo.join("\n")).unwrap();
    let doc = ok(a, "doc_create", json!({ "from": "text", "path": "memo.txt" }))["doc"].as_u64().unwrap();
    ok(a, "doc_save", json!({ "doc": doc, "path": "memo.pdf" }));
    doc
}

#[test]
fn the_result_reports_what_was_removed_without_its_content() {
    let dir = workdir("report");
    let mut a = auto(&dir);
    let doc = open_memo(&mut a, &dir);
    ok(&mut a, "redact_mark", json!({ "doc": doc, "find": SECRET }));
    let r = ok(&mut a, "redact_apply", json!({ "doc": doc }));
    assert_eq!((r["status"].as_str(), r["applied"].as_u64(), r["marks_pending"].as_u64()), (Some("complete"), Some(2), Some(0)));
    assert_eq!((r["report"]["marks"].as_u64(), r["report"]["pages"].as_u64()), (Some(2), Some(2)));
    assert!(r["report"]["text_glyphs"].as_u64() >= Some(20), "{r}");
    assert_eq!((r["checked_at_apply"].as_bool(), r["undo"].clone()), (Some(true), Value::Null), "history is purged");
    assert!(r.get("verified").is_none(), "no claim of a guarantee: {r}");
    assert!(r["caveats"].as_array().is_some_and(|c| c.len() == 2 && c[0].as_str().is_some_and(|t| t.contains("pixels"))), "{r}");
    // The independent proof, shareable: statuses and counts, per region, never the content.
    let proof = &r["proof"];
    assert_eq!(
        (proof["passed"].as_bool(), proof["survivors"].as_u64(), proof["unverifiable_regions"].as_u64()),
        (Some(true), Some(0), Some(0)),
        "{r}"
    );
    let regions = proof["regions"].as_array().unwrap();
    assert_eq!(regions.len(), 2, "{r}");
    assert!(regions.iter().all(|g| g["status"] == "Verified" && g["page"].as_u64().is_some() && g["area"].is_array()), "{r}");
    assert!(proof["surfaces"].as_array().is_some_and(|s| s.iter().any(|x| x["surface"] == "Info")), "{r}");
    // apply sanitizes too: the counts say what went.
    assert!(r["sanitized"]["total"].as_u64().is_some() && r["sanitized"]["counts"].is_object(), "{r}");
    assert!(r["sanitized"]["layers"].is_u64(), "layers are counted, never named: {r}");
    assert!(!r.to_string().contains(SECRET) && !r.to_string().contains("4711"), "{r}");
    assert!(texts(&mut a, doc).iter().all(|t| !t.contains("4711")), "the text cache followed the edit");
}

#[test]
fn applying_some_pages_is_reported_as_partial() {
    let dir = workdir("partial");
    let mut a = auto(&dir);
    let doc = open_memo(&mut a, &dir);
    ok(&mut a, "redact_mark", json!({ "doc": doc, "find": SECRET }));
    let r = ok(&mut a, "redact_apply", json!({ "doc": doc, "pages": [1] }));
    assert_eq!((r["status"].as_str(), r["applied"].as_u64(), r["marks_pending"].as_u64()), (Some("partial"), Some(1), Some(1)));
    let t = texts(&mut a, doc);
    assert!(!t[0].contains(SECRET) && t[2].contains(SECRET), "page 3 still has its content until its mark is applied");
}

#[test]
fn a_failed_apply_changes_nothing_and_names_no_content() {
    let dir = workdir("failed");
    let mut a = auto(&dir);
    let doc = open_memo(&mut a, &dir);
    ok(&mut a, "redact_mark", json!({ "doc": doc, "find": SECRET }));
    ok(&mut a, "redact_apply", json!({ "doc": doc }));
    // Nothing left to apply: an error, and the message is about marks, not content.
    let Err(ToolError::Failed(m)) = a.call("redact_apply", &json!({ "doc": doc })) else { panic!("expected a failure") };
    assert!(m.contains("no redaction marks") && !m.contains(SECRET), "{m}");
}

#[test]
fn redacted_output_does_not_replace_the_source_unless_asked() {
    let dir = workdir("save");
    let mut a = auto(&dir);
    let doc = open_memo(&mut a, &dir);
    let original = std::fs::read(dir.join("memo.pdf")).unwrap();
    ok(&mut a, "redact_mark", json!({ "doc": doc, "find": SECRET }));
    ok(&mut a, "redact_apply", json!({ "doc": doc }));
    // Saving in place (no path, or the same path) is refused and the source stays as it was.
    for args in [json!({ "doc": doc }), json!({ "doc": doc, "path": "memo.pdf" })] {
        let Err(ToolError::Failed(m)) = a.call("doc_save", &args) else { panic!("expected a refusal") };
        assert!(m.contains("overwrite_source"), "{m}");
    }
    assert_eq!(std::fs::read(dir.join("memo.pdf")).unwrap(), original);
    let r = ok(&mut a, "doc_save", json!({ "doc": doc, "path": "memo-redacted.pdf" }));
    assert_eq!(r["incremental"], false);
    let saved = std::fs::read(dir.join("memo-redacted.pdf")).unwrap();
    assert_eq!(saved.windows(5).filter(|w| *w == b"%%EOF").count(), 1, "a full rewrite, one revision");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.join("memo-redacted.pdf")).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(std::fs::read(dir.join("memo.pdf")).unwrap(), original, "the source is untouched");
    // Saved: the document now lives at the new file, and saving in place is ordinary again.
    ok(&mut a, "doc_save", json!({ "doc": doc }));
}

#[test]
fn overwriting_the_source_needs_explicit_consent() {
    let dir = workdir("overwrite");
    let mut a = auto(&dir);
    let doc = open_memo(&mut a, &dir);
    ok(&mut a, "redact_mark", json!({ "doc": doc, "find": SECRET }));
    ok(&mut a, "redact_apply", json!({ "doc": doc }));
    let r = ok(&mut a, "doc_save", json!({ "doc": doc, "overwrite_source": true }));
    assert_eq!(r["incremental"], false);
    let saved = std::fs::read(dir.join("memo.pdf")).unwrap();
    assert_eq!(saved.windows(5).filter(|w| *w == b"%%EOF").count(), 1);
    let again = ok(&mut a, "doc_open", json!({ "path": "memo.pdf" }))["doc"].as_u64().unwrap();
    assert!(texts(&mut a, again).iter().all(|t| !t.contains("4711")));
}

#[test]
fn sanitizing_is_not_saved_over_the_source_without_consent_either() {
    let dir = workdir("sanitize-save");
    let mut a = auto(&dir);
    let doc = open_memo(&mut a, &dir);
    let original = std::fs::read(dir.join("memo.pdf")).unwrap();
    ok(&mut a, "doc_remove_hidden", json!({ "doc": doc }));
    let Err(ToolError::Failed(m)) = a.call("doc_save", &json!({ "doc": doc })) else { panic!("expected a refusal") };
    assert!(m.contains("overwrite_source"), "{m}");
    assert_eq!(std::fs::read(dir.join("memo.pdf")).unwrap(), original);
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "memo-clean.pdf" }));
}
