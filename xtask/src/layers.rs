//! Dependency layering rules (plan/architecture.md §3), enforced by `cargo xtask layers`.
//!
//! The rule engine works on a small, metadata-independent model so it can be unit-tested;
//! `from_metadata` builds that model from `cargo metadata`. (Structure shared with PhotoCraft.)

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Regular layered crate.
    Layer(u8),
    /// Layer 0/1 core that may depend only on the workspace crates listed in `STANDALONE_DEPS`.
    Standalone(u8),
    /// Test tooling: dev-dependency only for other crates.
    Testkit,
    /// Binaries, build tooling: exempt.
    Exempt,
}

impl Class {
    fn layer(self) -> Option<u8> {
        match self {
            Class::Layer(l) | Class::Standalone(l) => Some(l),
            Class::Testkit => Some(7),
            Class::Exempt => None,
        }
    }
}

/// Every workspace crate (package name without the `pdfcraft-` prefix) and its layer.
pub const TABLE: &[(&str, Class)] = &[
    // L0 foundation (standalone, publishable)
    ("geom", Class::Standalone(0)),
    ("filters", Class::Standalone(0)),
    ("crypt", Class::Standalone(0)),
    ("arlington", Class::Standalone(0)),
    // L1 object layer
    ("cos", Class::Standalone(1)),
    // L2 core model
    ("content", Class::Layer(2)),
    ("model", Class::Layer(2)),
    ("color", Class::Layer(2)),
    ("fonts", Class::Layer(2)),
    ("ops", Class::Layer(2)),
    // L3 core services
    ("render", Class::Layer(3)),
    ("text", Class::Layer(3)),
    ("annot", Class::Layer(3)),
    ("forms", Class::Layer(3)),
    ("js", Class::Layer(3)),
    ("xfa", Class::Layer(3)),
    ("ml", Class::Layer(3)),
    // L4 features
    ("edit", Class::Layer(4)),
    ("organize", Class::Layer(4)),
    ("security", Class::Layer(4)),
    ("redact", Class::Layer(4)),
    ("xfdf", Class::Layer(4)),
    ("sign", Class::Layer(4)),
    ("ocr", Class::Layer(4)),
    ("create", Class::Layer(4)),
    ("export", Class::Layer(4)),
    ("optimize", Class::Layer(4)),
    ("preflight", Class::Layer(4)),
    ("prepress", Class::Layer(4)),
    ("a11y", Class::Layer(4)),
    ("compare", Class::Layer(4)),
    ("measure", Class::Layer(4)),
    ("search", Class::Layer(4)),
    ("print", Class::Layer(4)),
    ("media", Class::Layer(4)),
    ("ai", Class::Layer(4)),
    // L5 interaction
    ("viewport", Class::Layer(5)),
    ("tools", Class::Layer(5)),
    // L6 façade
    ("engine", Class::Layer(6)),
    // L7 platform + frontends
    ("platform", Class::Layer(7)),
    ("ui-common", Class::Layer(7)),
    ("ui-egui", Class::Layer(7)),
    ("automation", Class::Layer(7)),
    ("sdk", Class::Layer(7)),
    ("rest", Class::Layer(7)),
    // test support
    ("testkit", Class::Testkit),
    ("oracle", Class::Testkit),
    // L8 apps + tooling
    ("pdfcraft", Class::Exempt),
    ("cli", Class::Exempt),
    ("web", Class::Exempt),
    ("xtask", Class::Exempt),
];

/// Workspace crates a standalone crate may use (architecture §3 rule 3: `cos → filters, crypt, geom`).
pub const STANDALONE_DEPS: &[(&str, &[&str])] = &[("cos", &["filters", "crypt", "geom"])];

/// Allowed same-layer edges (architecture §3 rule 1). `forms → js` is deliberately absent.
pub const SIDEWAYS: &[(&str, &str)] = &[
    ("redact", "edit"),
    ("a11y", "preflight"),
    ("prepress", "preflight"),
    ("export", "ocr"),
    ("sign", "security"),
    ("ui-egui", "ui-common"),
    ("ui-egui", "platform"),
    ("automation", "platform"),
    ("sdk", "automation"),
    ("rest", "sdk"),
    ("rest", "automation"),
];

/// External crates that constitute a UI toolkit / windowing dependency (prefix match with `*`).
pub const UI_CRATES: &[&str] = &["egui", "eframe", "winit", "egui_kittest", "egui_extras", "egui_dock", "egui_tiles", "rfd", "wgpu*"];
pub const UI_MIN_LAYER: u8 = 7;

pub fn short_name(pkg: &str) -> &str {
    pkg.strip_prefix("pdfcraft-").unwrap_or(pkg)
}

pub fn classify(pkg: &str) -> Option<Class> {
    let s = short_name(pkg);
    TABLE.iter().find(|(n, _)| *n == s).map(|(_, c)| *c)
}

fn is_ui_crate(name: &str) -> bool {
    UI_CRATES.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == *p,
    })
}

fn sideways_allowed(from: &str, to: &str) -> bool {
    SIDEWAYS.contains(&(short_name(from), short_name(to)))
}

fn standalone_allowed(from: &str, to: &str) -> bool {
    STANDALONE_DEPS.iter().any(|(f, ok)| *f == short_name(from) && ok.contains(&short_name(to)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone)]
pub struct Dep {
    pub name: String,
    pub kind: DepKind,
    pub workspace: bool,
}

#[derive(Debug, Clone)]
pub struct Crate {
    pub name: String,
    pub deps: Vec<Dep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    Unregistered { krate: String },
    Upward { krate: String, dep: String, from: u8, to: u8, kind: DepKind },
    StandaloneHasWorkspaceDep { krate: String, dep: String },
    TestkitAsNormalDep { krate: String, dep: String },
    UiBelowL7 { krate: String, dep: String, layer: u8 },
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::Unregistered { krate } => {
                write!(f, "{krate}: unknown workspace crate; register it in xtask/src/layers.rs TABLE (plan/architecture.md §3)")
            }
            Violation::Upward { krate, dep, from, to, kind } => {
                write!(f, "{krate} (L{from}) -> {dep} (L{to}) [{kind:?}]: may only depend on lower layers (or an allowed sideways edge)")
            }
            Violation::StandaloneHasWorkspaceDep { krate, dep } => write!(f, "{krate}: standalone crate must not depend on workspace crate {dep}"),
            Violation::TestkitAsNormalDep { krate, dep } => write!(f, "{krate}: {dep} may only be a dev-dependency"),
            Violation::UiBelowL7 { krate, dep, layer } => {
                write!(f, "{krate} (L{layer}) depends on UI crate `{dep}`; UI toolkits are only allowed in L7+")
            }
        }
    }
}

pub fn check(crates: &[Crate]) -> Vec<Violation> {
    let mut out = Vec::new();
    for c in crates {
        let Some(class) = classify(&c.name) else {
            out.push(Violation::Unregistered { krate: c.name.clone() });
            continue;
        };
        if class == Class::Exempt {
            continue;
        }
        let layer = class.layer().unwrap_or(0);
        for d in &c.deps {
            if d.name == c.name {
                continue;
            }
            if d.workspace {
                if matches!(class, Class::Standalone(_)) && !standalone_allowed(&c.name, &d.name) {
                    out.push(Violation::StandaloneHasWorkspaceDep { krate: c.name.clone(), dep: d.name.clone() });
                    continue;
                }
                match classify(&d.name) {
                    None => {}
                    Some(Class::Testkit) if d.kind != DepKind::Dev && class != Class::Testkit => {
                        out.push(Violation::TestkitAsNormalDep { krate: c.name.clone(), dep: d.name.clone() });
                    }
                    Some(Class::Testkit) => {}
                    Some(dc) => {
                        let to = dc.layer().unwrap_or(u8::MAX);
                        let ok = to < layer || (to == layer && sideways_allowed(&c.name, &d.name));
                        if !ok {
                            out.push(Violation::Upward { krate: c.name.clone(), dep: d.name.clone(), from: layer, to: to.min(8), kind: d.kind });
                        }
                    }
                }
            } else if layer < UI_MIN_LAYER && is_ui_crate(&d.name) && !(class == Class::Testkit || d.kind == DepKind::Dev && d.name == "egui_kittest")
            {
                out.push(Violation::UiBelowL7 { krate: c.name.clone(), dep: d.name.clone(), layer });
            }
        }
    }
    out.sort_by_key(|v| v.to_string());
    out.dedup();
    out
}

pub fn from_metadata(meta: &Value) -> Result<Vec<Crate>, String> {
    let pkgs = meta["packages"].as_array().ok_or("metadata: no packages array")?;
    let members: Vec<&str> = pkgs.iter().filter_map(|p| p["name"].as_str()).collect();
    let mut out = Vec::new();
    for p in pkgs {
        let name = p["name"].as_str().ok_or("package without name")?.to_owned();
        let mut deps = Vec::new();
        for d in p["dependencies"].as_array().into_iter().flatten() {
            let dname = d["name"].as_str().unwrap_or_default().to_owned();
            let kind = match d["kind"].as_str() {
                Some("dev") => DepKind::Dev,
                Some("build") => DepKind::Build,
                _ => DepKind::Normal,
            };
            let workspace = members.contains(&dname.as_str());
            deps.push(Dep { name: dname, kind, workspace });
        }
        out.push(Crate { name, deps });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn describe(class: Option<Class>) -> String {
    match class {
        Some(Class::Layer(l)) => format!("L{l}"),
        Some(Class::Standalone(l)) => format!("L{l} standalone"),
        Some(Class::Testkit) => "test support".into(),
        Some(Class::Exempt) => "exempt".into(),
        None => "UNREGISTERED".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::DepKind::*;
    use super::*;

    fn c(name: &str, deps: &[(&str, DepKind, bool)]) -> Crate {
        Crate { name: name.into(), deps: deps.iter().map(|(n, k, w)| Dep { name: (*n).into(), kind: *k, workspace: *w }).collect() }
    }

    #[test]
    fn current_graph_shape_passes() {
        let g = [
            c("pdfcraft-geom", &[]),
            c("pdfcraft-render", &[("hayro", Normal, false), ("lopdf", Normal, false)]),
            c("pdfcraft-engine", &[("pdfcraft-render", Normal, true)]),
            c("pdfcraft-ui-egui", &[("pdfcraft-engine", Normal, true), ("egui", Normal, false), ("egui_kittest", Dev, false)]),
            c("pdfcraft", &[("pdfcraft-ui-egui", Normal, true)]),
        ];
        assert!(check(&g).is_empty(), "{:?}", check(&g));
    }

    #[test]
    fn upward_and_sideways_edges_flagged() {
        let v = check(&[c("pdfcraft-model", &[("pdfcraft-engine", Normal, true)])]);
        assert!(matches!(v[..], [Violation::Upward { from: 2, to: 6, .. }]));
        // forms -> js must go through the ActionRunner trait, never directly.
        let v = check(&[c("pdfcraft-forms", &[("pdfcraft-js", Normal, true)])]);
        assert!(matches!(v[..], [Violation::Upward { from: 3, to: 3, .. }]));
    }

    #[test]
    fn listed_sideways_edges_allowed() {
        assert!(check(&[c("pdfcraft-redact", &[("pdfcraft-edit", Normal, true)])]).is_empty());
        assert!(!check(&[c("pdfcraft-edit", &[("pdfcraft-redact", Normal, true)])]).is_empty());
    }

    #[test]
    fn cos_may_use_only_its_listed_foundation() {
        assert!(check(&[c("pdfcraft-cos", &[("pdfcraft-filters", Normal, true), ("pdfcraft-crypt", Normal, true)])]).is_empty());
        let v = check(&[c("pdfcraft-cos", &[("pdfcraft-arlington", Normal, true)])]);
        assert!(matches!(v[..], [Violation::StandaloneHasWorkspaceDep { .. }]));
        let v = check(&[c("pdfcraft-filters", &[("pdfcraft-geom", Normal, true)])]);
        assert!(matches!(v[..], [Violation::StandaloneHasWorkspaceDep { .. }]));
    }

    #[test]
    fn ui_crates_below_l7_flagged() {
        for dep in ["egui", "eframe", "winit", "rfd", "wgpu", "wgpu-core", "egui_dock"] {
            let v = check(&[c("pdfcraft-engine", &[(dep, Normal, false)])]);
            assert!(matches!(v[..], [Violation::UiBelowL7 { layer: 6, .. }]), "{dep}");
        }
        assert!(check(&[c("pdfcraft-platform", &[("winit", Normal, false)])]).is_empty());
    }

    #[test]
    fn test_support_only_as_dev_dependency() {
        let v = check(&[c("pdfcraft-cos", &[("pdfcraft-testkit", Normal, true)])]);
        assert!(!v.is_empty());
        assert!(check(&[c("pdfcraft-render", &[("pdfcraft-oracle", Dev, true)])]).is_empty());
        let v = check(&[c("pdfcraft-render", &[("pdfcraft-oracle", Normal, true)])]);
        assert!(matches!(v[..], [Violation::TestkitAsNormalDep { .. }]));
    }

    #[test]
    fn unregistered_crate_is_error() {
        let v = check(&[c("pdfcraft-mystery", &[])]);
        assert!(matches!(&v[..], [Violation::Unregistered { krate }] if krate == "pdfcraft-mystery"));
    }

    #[test]
    fn apps_exempt() {
        for app in ["pdfcraft", "pdfcraft-cli", "pdfcraft-web", "xtask"] {
            assert!(check(&[c(app, &[("egui", Normal, false), ("pdfcraft-ui-egui", Normal, true)])]).is_empty());
        }
    }
}
