//! XFA scripting in the engine: a form's scripted events (initialize and calculate on open,
//! exit, validate and calculate after a field changes, click for buttons) run through
//! [`pdfcraft_js::xfa`] over the live form tree [`pdfcraft_xfa::form_tree`] builds, and what
//! they change or ask for is applied here: values go to the fields (and so to the datasets),
//! presence and access changes become overrides kept in the PDF, added or removed rows change
//! the datasets, and any of those lays the form out again.

use std::collections::HashMap;
use std::sync::Arc;

use pdfcraft_js::formcalc::run_formcalc_within;
use pdfcraft_js::xfa::{XfaDoc, XfaEffect, XfaEvent, XfaKind, XfaNode, run_xfa_within};
use pdfcraft_xfa::model::Template;
use pdfcraft_xfa::{FormNode, LiveForm, NodeKind, ScriptEvent};

use crate::js::JsOutput;
use crate::js::Request;

/// Most scripts run for one event (calculates of a whole form, say).
const MAX_SCRIPTS_PER_EVENT: usize = 2_000;
/// Most times the form is laid out again for one event (a script adding rows in a loop).
const MAX_RELAYOUTS_PER_EVENT: usize = 50;
/// Most message boxes, console lines and errors kept for one event.
const MAX_ALERTS_PER_EVENT: usize = 100;
const MAX_CONSOLE_PER_EVENT: usize = 1_000;
const MAX_ERRORS_PER_EVENT: usize = 100;
/// Time the scripts of one event may take in all before the rest are skipped: opening (and
/// the calculations every change runs) is held tighter than a click.
const OPEN_BUDGET_MS: u64 = 3_000;
const EVENT_BUDGET_MS: u64 = 5_000;

/// Engine limits for one script: initialize, calculate and validate scripts run without the
/// user asking (on open, on every change), so they get fewer loop iterations than a click.
fn limits_for(activity: &str) -> pdfcraft_js::Limits {
    match activity {
        "click" | "change" | "exit" | "enter" => pdfcraft_js::Limits::default(),
        _ => pdfcraft_js::Limits { loop_iterations: 100_000, recursion: 64 },
    }
}

/// Longest one script may run before it is abandoned (it can't be interrupted, only left
/// behind): scripts that run unasked are held tighter than a click's.
fn timeout_for(activity: &str) -> std::time::Duration {
    std::time::Duration::from_millis(match activity {
        "click" | "change" | "exit" | "enter" => 3_000,
        _ => 1_000,
    })
}

/// Requests only a click may make (anything else asking is noted and ignored).
fn click_only(e: &XfaEffect) -> Option<&'static str> {
    match e {
        XfaEffect::Print => Some("print"),
        XfaEffect::SaveAs => Some("save"),
        XfaEffect::LaunchUrl(_) => Some("open a link"),
        XfaEffect::SetFocus(_) => Some("move the focus"),
        _ => None,
    }
}

/// A wall clock where there is one (not in the browser build, where the script and relayout
/// caps alone bound an event).
#[derive(Clone, Copy)]
struct Clock {
    #[cfg(not(target_arch = "wasm32"))]
    started: std::time::Instant,
    budget_ms: u64,
}

impl Clock {
    fn start(budget_ms: u64) -> Self {
        Clock {
            #[cfg(not(target_arch = "wasm32"))]
            started: std::time::Instant::now(),
            budget_ms,
        }
    }

    fn spent(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.started.elapsed() >= std::time::Duration::from_millis(self.budget_ms)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = self.budget_ms;
            false
        }
    }
}

/// What the scripts of an event changed in the form (for the note opening leaves).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Changes {
    pub values: usize,
    pub rows: usize,
    pub visibility: usize,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.values == 0 && self.rows == 0 && self.visibility == 0
    }

    /// "3 values, 1 row" — what changed, for a note.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        let n = |k: usize, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
        if self.values > 0 {
            parts.push(n(self.values, "field value", "field values"));
        }
        if self.rows > 0 {
            parts.push(n(self.rows, "added or removed row", "added or removed rows"));
        }
        if self.visibility > 0 {
            parts.push(n(self.visibility, "shown, hidden or locked object", "shown, hidden or locked objects"));
        }
        parts.join(", ")
    }
}

/// What a run of the form's scripts shares with the edit around it.
pub(crate) struct XfaRun<'a> {
    pub page_count: usize,
    pub out: &'a mut JsOutput,
    /// The datasets stream this edit already wrote, replaced in place by later writes.
    pub datasets: Option<pdfcraft_cos::ObjRef>,
    /// A script ran past its time limit and was abandoned: the form's scripts should stay off.
    pub ran_away: bool,
}

fn to_js(n: &FormNode) -> XfaNode {
    XfaNode {
        name: n.name.clone(),
        som: n.som.clone(),
        kind: n.kind.map(|k| match k {
            NodeKind::Subform => XfaKind::Subform,
            NodeKind::Field => XfaKind::Field,
            NodeKind::ExclGroup => XfaKind::ExclGroup,
            NodeKind::Draw => XfaKind::Draw,
            NodeKind::Area => XfaKind::Area,
        }),
        value: n.value.clone(),
        numeric: n.numeric,
        presence: n.presence.clone(),
        access: n.access.clone(),
        repeatable: n.repeatable,
        occur_min: n.occur_min,
        occur_max: n.occur_max,
        index: n.index,
        children: n.children.iter().map(to_js).collect(),
    }
}

fn find_container<'a>(n: &'a FormNode, som: &str, depth: usize) -> Option<&'a FormNode> {
    if depth > 64 {
        return None;
    }
    if n.som == som && n.kind != Some(NodeKind::Field) {
        return Some(n);
    }
    n.children.iter().find_map(|c| find_container(c, som, depth + 1))
}

fn count_instances(root: &FormNode, parent_som: &str, name: &str) -> usize {
    let parent = if parent_som.is_empty() { Some(root) } else { find_container(root, parent_som, 0) };
    parent.map_or(0, |p| p.children.iter().filter(|c| c.name == name).count())
}

/// An instance SOM (`…table[0].row[2]`) split into its parent's data path (areas left out,
/// see [`pdfcraft_xfa::data_path`]) and SOM, its name and its index.
fn split_instance(tpl: &Template, som: &str) -> Option<(pdfcraft_xfa::DataPath, String, String, usize)> {
    let mut path = pdfcraft_xfa::data_path(tpl, som);
    let (name, index) = path.pop()?;
    let parent_som = som.rsplit_once('.').map(|(p, _)| p).unwrap_or("").to_string();
    Some((path, parent_som, name, index))
}

/// A SOM path without its indexes (`form.page1.qty`), for matching names scripts pass.
fn plain_som(som: &str) -> String {
    let mut out = String::with_capacity(som.len());
    let mut skip = false;
    for c in som.chars() {
        match c {
            '[' => skip = true,
            ']' => skip = false,
            _ if !skip => out.push(c),
            _ => {}
        }
    }
    out
}

/// Does `name` from a script (`qty`, `page1.qty`, `xfa.form.form.page1.qty`, or a container
/// such as `table`) name this SOM or one of its ancestors?
fn som_matches(som: &str, name: &str) -> bool {
    let plain = plain_som(som);
    let n = plain_som(name.trim());
    let n = n.strip_prefix("xfa.form.").or_else(|| n.strip_prefix("$form.")).unwrap_or(&n);
    let want: Vec<&str> = n.split('.').filter(|s| !s.is_empty()).collect();
    let have: Vec<&str> = plain.split('.').filter(|s| !s.is_empty()).collect();
    !want.is_empty() && have.windows(want.len()).any(|w| w == want.as_slice())
}

/// A generated field as the runner tracks it: kind, value and on state.
#[derive(Clone)]
struct FieldInfo {
    kind: pdfcraft_forms::FieldKind,
    value: Vec<String>,
    on_state: String,
    /// List boxes that take several values (one per line in the data).
    multi: bool,
}

fn field_infos(doc: &pdfcraft_cos::Document) -> HashMap<String, FieldInfo> {
    pdfcraft_forms::fields(doc)
        .into_iter()
        .map(|f| {
            let on_state = f.widgets.first().and_then(|w| w.on_state.clone()).unwrap_or_else(|| "1".into());
            let multi = f.kind == pdfcraft_forms::FieldKind::List && f.has(pdfcraft_forms::flags::MULTI_SELECT);
            (f.name, FieldInfo { kind: f.kind, value: f.value, on_state, multi })
        })
        .collect()
}

/// The data text of a field's value.
fn data_text(info: &FieldInfo) -> String {
    match info.kind {
        pdfcraft_forms::FieldKind::CheckBox => {
            if info.value.is_empty() {
                String::new()
            } else {
                info.on_state.clone()
            }
        }
        _ => info.value.join("\n"),
    }
}

/// Runs the scripts of one event (opening, a change, a click) against one document: what
/// they change is kept in memory (data, field values, overrides) and written once, when the
/// event ends or the form has to be laid out again.
struct Runner<'a, 'b> {
    doc: &'a mut pdfcraft_cos::Document,
    tpl: &'a Template,
    run: &'a mut XfaRun<'b>,
    clock: Clock,
    runs: usize,
    skipped: usize,
    relayouts: usize,
    relayouts_skipped: bool,
    dropped_output: usize,
    truncated_warned: bool,
    data: Option<pdfcraft_xfa::DataNode>,
    ov: pdfcraft_xfa::Overrides,
    ov_dirty: bool,
    pending: Vec<pdfcraft_xfa::DataOp>,
    /// Field values to write, by name (the last wins).
    field_changes: std::collections::BTreeMap<String, Vec<String>>,
    by_som: Option<HashMap<String, String>>,
    fields: Option<HashMap<String, FieldInfo>>,
    /// The live form as last built; dropped when a script changed the form.
    cached: Option<LiveForm>,
    changes: Changes,
}

impl<'a, 'b> Runner<'a, 'b> {
    fn new(doc: &'a mut pdfcraft_cos::Document, tpl: &'a Template, run: &'a mut XfaRun<'b>, budget_ms: u64) -> Self {
        let data = pdfcraft_xfa::data_of(doc);
        let ov = pdfcraft_xfa::overrides(doc);
        Runner {
            doc,
            tpl,
            run,
            clock: Clock::start(budget_ms),
            runs: 0,
            skipped: 0,
            relayouts: 0,
            relayouts_skipped: false,
            dropped_output: 0,
            truncated_warned: false,
            data,
            ov,
            ov_dirty: false,
            pending: Vec::new(),
            field_changes: Default::default(),
            by_som: None,
            fields: None,
            cached: None,
            changes: Changes::default(),
        }
    }

    fn live(&mut self) -> &LiveForm {
        if self.cached.is_none() {
            let live = pdfcraft_xfa::form_tree(self.tpl, self.data.as_ref(), &self.ov);
            if live.truncated && !self.truncated_warned {
                self.truncated_warned = true;
                self.error(format!(
                    "The form has more than {} objects or 20000 scripted events; its scripts see only the first ones",
                    pdfcraft_xfa::MAX_FORM_NODES
                ));
            }
            self.cached = Some(live);
        }
        self.cached.get_or_insert_with(Default::default)
    }

    fn by_som(&mut self) -> &HashMap<String, String> {
        if self.by_som.is_none() {
            self.by_som = Some(pdfcraft_xfa::fields_by_som(self.doc));
        }
        self.by_som.get_or_insert_with(Default::default)
    }

    fn fields(&mut self) -> &mut HashMap<String, FieldInfo> {
        if self.fields.is_none() {
            self.fields = Some(field_infos(self.doc));
        }
        self.fields.get_or_insert_with(Default::default)
    }

    fn error(&mut self, e: String) {
        if self.run.out.errors.len() < MAX_ERRORS_PER_EVENT {
            self.run.out.errors.push(e);
        } else {
            self.dropped_output += 1;
        }
    }

    fn console(&mut self, lines: impl IntoIterator<Item = String>) {
        for l in lines {
            if self.run.out.console.len() < MAX_CONSOLE_PER_EVENT {
                self.run.out.console.push(l);
            } else {
                self.dropped_output += 1;
            }
        }
    }

    fn alert(&mut self, m: String) {
        if self.run.out.alerts.len() < MAX_ALERTS_PER_EVENT {
            self.run.out.alerts.push(m);
        } else {
            self.dropped_output += 1;
        }
    }

    /// Change the data (and the in-memory copy scripts read).
    fn data_op(&mut self, op: pdfcraft_xfa::DataOp) {
        self.data.get_or_insert_with(Default::default).apply(&op);
        self.pending.push(op);
        self.cached = None;
    }

    /// Give the object at `som` the value `value` (field `name` when it has a widget). Returns
    /// whether it changed.
    fn set_value(&mut self, som: &str, value: &str) -> bool {
        let path = pdfcraft_xfa::data_path(self.tpl, som);
        let name = self.by_som().get(som).cloned();
        let Some(name) = name else {
            // No widget (a hidden field, a draw, a row not laid out yet): the data alone.
            if self.data.as_ref().and_then(|d| d.text_at(&path)) == Some(value) {
                return false;
            }
            self.data_op(pdfcraft_xfa::DataOp::Value { path, text: value.to_string() });
            self.changes.values += 1;
            return true;
        };
        let Some(info) = self.fields().get(&name).cloned() else { return false };
        use pdfcraft_forms::FieldKind as K;
        let new_value: Vec<String> = match info.kind {
            K::CheckBox => {
                if pdfcraft_xfa::data::is_on(value, &info.on_state) || value == info.on_state {
                    vec![info.on_state.clone()]
                } else {
                    Vec::new()
                }
            }
            K::Radio => {
                let v = value.trim();
                if v.is_empty() { Vec::new() } else { vec![v.to_string()] }
            }
            // Read-only fields still take values from scripts (calculated totals are read-only).
            K::PushButton | K::Signature => return false,
            K::List if info.multi => value.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect(),
            _ => vec![value.to_string()],
        };
        let same = match info.kind {
            K::CheckBox => info.value.is_empty() == new_value.is_empty(),
            K::Radio => info.value.first() == new_value.first(),
            K::List if info.multi => info.value == new_value,
            _ => info.value.first().map_or(value.is_empty(), |v| v == value),
        };
        if same {
            return false;
        }
        let updated = FieldInfo { value: new_value.clone(), ..info };
        let text = data_text(&updated);
        self.fields().insert(name.clone(), updated);
        self.field_changes.insert(name, new_value);
        self.data_op(pdfcraft_xfa::DataOp::Value { path, text });
        self.changes.values += 1;
        true
    }

    /// Write what the scripts changed so far: the data as one rewrite, the field values, the
    /// overrides.
    fn flush(&mut self) -> Result<(), String> {
        let mut wrote = false;
        if !self.pending.is_empty() {
            let ops = std::mem::take(&mut self.pending);
            let r = pdfcraft_xfa::write_data_ops(self.doc, &ops, self.run.datasets).map_err(|e| e.to_string())?;
            if r.stream.is_some() {
                self.run.datasets = r.stream;
            }
            self.console(r.warnings);
            wrote = true;
        }
        if !self.field_changes.is_empty() {
            let changes: Vec<pdfcraft_forms::FieldChange> = std::mem::take(&mut self.field_changes)
                .into_iter()
                .map(|(name, value)| pdfcraft_forms::FieldChange { name, value: Some(value), ..Default::default() })
                .collect();
            pdfcraft_forms::apply_script_changes(self.doc, &changes, &mut pdfcraft_forms::NoScripts).map_err(|e| e.to_string())?;
            self.fields = None;
        }
        if self.ov_dirty {
            pdfcraft_xfa::set_overrides(self.doc, &self.ov).map_err(|e| e.to_string())?;
            self.ov_dirty = false;
        }
        if wrote {
            // What the packet now holds (a value it could not take is reported above).
            self.data = pdfcraft_xfa::data_of(self.doc);
            self.cached = None;
        }
        Ok(())
    }

    /// Lay the form out again and give the new widgets their appearances.
    fn relayout(&mut self) -> Result<(), String> {
        if self.relayouts >= MAX_RELAYOUTS_PER_EVENT {
            if !self.relayouts_skipped {
                self.relayouts_skipped = true;
                self.error(format!(
                    "The form's scripts asked to lay it out again more than {MAX_RELAYOUTS_PER_EVENT} times; the form is laid out once more when they finish"
                ));
            }
            return Ok(());
        }
        self.relayouts += 1;
        self.flush()?;
        let report = pdfcraft_xfa::rerender(self.doc, self.tpl).map_err(|e| e.to_string())?;
        self.console(report.warnings);
        for f in pdfcraft_forms::fields(self.doc) {
            pdfcraft_forms::redraw_field(self.doc, &f.name).map_err(|e| format!("{}: {e}", f.name))?;
        }
        self.by_som = None;
        self.fields = None;
        self.cached = None;
        Ok(())
    }

    /// `xfa.host.resetData`: the fields back to their defaults, in the form and the data.
    fn reset(&mut self, names: &[String]) -> Result<(), String> {
        self.flush()?;
        let by_som = self.by_som().clone();
        let listed: Option<Vec<String>> = if names.is_empty() {
            None
        } else {
            Some(by_som.iter().filter(|(som, _)| names.iter().any(|n| som_matches(som, n))).map(|(_, f)| f.clone()).collect())
        };
        if listed.as_ref().is_some_and(|l| l.is_empty()) {
            return Ok(());
        }
        pdfcraft_forms::reset(self.doc, listed.as_deref()).map_err(|e| e.to_string())?;
        self.fields = None;
        let infos = self.fields().clone();
        for (som, name) in &by_som {
            if listed.as_ref().is_some_and(|l| !l.contains(name)) {
                continue;
            }
            let Some(info) = infos.get(name) else { continue };
            if matches!(info.kind, pdfcraft_forms::FieldKind::PushButton | pdfcraft_forms::FieldKind::Signature) {
                continue;
            }
            let path = pdfcraft_xfa::data_path(self.tpl, som);
            let text = data_text(info);
            if self.data.as_ref().and_then(|d| d.text_at(&path)).unwrap_or("") != text {
                self.data_op(pdfcraft_xfa::DataOp::Value { path, text });
                self.changes.values += 1;
            }
        }
        self.cached = None;
        Ok(())
    }

    /// Apply what a script of `ev` did. Returns whether the form has to be laid out again.
    fn apply_effects(&mut self, ev: &ScriptEvent, effects: &[XfaEffect]) -> Result<bool, String> {
        let mut relayout = false;
        // Instances of each repeating subform before the script ran, as the effects leave them.
        let mut counts: HashMap<(String, String), usize> = HashMap::new();
        {
            // A copy of the shared reference: `live()` borrows `self` mutably.
            let tpl = self.tpl;
            let live = self.live();
            for e in effects {
                if let XfaEffect::AddInstance { som } | XfaEffect::RemoveInstance { som } = e
                    && let Some((_, parent_som, name, _)) = split_instance(tpl, som)
                {
                    let have = count_instances(&live.root, &parent_som, &name);
                    counts.entry((parent_som, name)).or_insert(have);
                }
            }
        }
        for e in effects {
            // Only a click script may print, save, open links or move the focus (not the
            // calculations a click sets off, nor anything run on open or on a change).
            if let Some(what) = click_only(e)
                && ev.activity != "click"
            {
                let note = format!("{} ({}): a script may only {what} when a button is clicked; ignored", ev.som, ev.activity);
                self.console([note]);
                continue;
            }
            match e {
                XfaEffect::SetValue { som, value } => {
                    self.set_value(som, value);
                }
                XfaEffect::SetPresence { som, presence } => {
                    if self.ov.presence.insert(som.clone(), presence.clone()).as_ref() != Some(presence) {
                        self.ov_dirty = true;
                        self.changes.visibility += 1;
                        self.cached = None;
                        relayout = true;
                    }
                }
                XfaEffect::SetAccess { som, access } => {
                    if self.ov.access.insert(som.clone(), access.clone()).as_ref() != Some(access) {
                        self.ov_dirty = true;
                        self.changes.visibility += 1;
                        self.cached = None;
                        relayout = true;
                    }
                }
                XfaEffect::AddInstance { som } | XfaEffect::RemoveInstance { som } => {
                    let Some((parent, parent_som, name, index)) = split_instance(self.tpl, som) else { continue };
                    let key = (parent_som, name.clone());
                    let have = counts.get(&key).copied().unwrap_or(0);
                    if matches!(e, XfaEffect::AddInstance { .. }) {
                        counts.insert(key, have + 1);
                        self.data_op(pdfcraft_xfa::DataOp::Instances { parent, name, count: (index + 1).max(have + 1) });
                    } else {
                        // The data holds every shown instance before one is taken out.
                        counts.insert(key, have.saturating_sub(1));
                        self.data_op(pdfcraft_xfa::DataOp::Instances { parent: parent.clone(), name: name.clone(), count: have });
                        let mut path = parent;
                        path.push((name, index));
                        self.data_op(pdfcraft_xfa::DataOp::Remove(path));
                    }
                    self.changes.rows += 1;
                    relayout = true;
                }
                XfaEffect::MessageBox(m) => self.alert(m.clone()),
                XfaEffect::ResetData(names) => self.reset(names)?,
                XfaEffect::Print => self.run.out.requests.push(Request::Print),
                XfaEffect::SaveAs => self.run.out.requests.push(Request::SaveAs),
                XfaEffect::LaunchUrl(u) => self.run.out.requests.push(Request::LaunchUrl(u.clone())),
                XfaEffect::SetFocus(som) => {
                    if let Some(name) = self.by_som().get(som).cloned() {
                        self.run.out.requests.push(Request::Focus(name));
                    }
                }
                XfaEffect::Beep => self.run.out.requests.push(Request::Beep),
                XfaEffect::Recalculate => {}
                XfaEffect::Relayout => relayout = true,
            }
        }
        Ok(relayout)
    }

    /// Run one event script against the form as it is now. `None`: it was skipped (the
    /// event's budget is spent).
    fn run(&mut self, ev: &ScriptEvent, new_text: &str) -> Result<Option<pdfcraft_js::xfa::XfaOutcome>, String> {
        if self.runs >= MAX_SCRIPTS_PER_EVENT || self.clock.spent() || self.run.ran_away {
            self.skipped += 1;
            return Ok(None);
        }
        self.runs += 1;
        let event = XfaEvent { activity: ev.activity.clone(), target: ev.som.clone(), new_text: new_text.to_string(), prev_text: String::new() };
        let doc_info = XfaDoc { file_name: String::new(), page: 0, page_count: self.run.page_count };
        let root_js = to_js(&self.live().root);
        let (limits, timeout) = (limits_for(&ev.activity), timeout_for(&ev.activity));
        let o = if ev.formcalc {
            run_formcalc_within(&ev.script, &event, &doc_info, root_js, limits, timeout)
        } else {
            run_xfa_within(&ev.script, &event, &doc_info, root_js, limits, timeout)
        };
        if o.abandoned {
            self.run.ran_away = true;
        }
        self.console(o.console.iter().cloned());
        for n in &o.notes {
            self.error(format!("{} ({}): {n}", ev.som, ev.activity));
        }
        if let Some(e) = &o.error {
            self.error(format!("{} ({}): {e}", ev.som, ev.activity));
            return Ok(Some(o));
        }
        if self.apply_effects(ev, &o.effects)? {
            self.relayout()?;
        }
        Ok(Some(o))
    }

    /// Every calculate script, in document order: the field takes the script's result unless
    /// the script set a value itself.
    fn calculates(&mut self) -> Result<(), String> {
        let events: Vec<ScriptEvent> = self.live().events.iter().filter(|e| e.activity == "calculate").cloned().collect();
        for ev in &events {
            let Some(o) = self.run(ev, "")? else { continue };
            if o.error.is_some() || o.effects.iter().any(|e| matches!(e, XfaEffect::SetValue { som, .. } if *som == ev.som)) {
                continue;
            }
            if let Some(result) = o.result {
                self.set_value(&ev.som, &result);
            }
        }
        Ok(())
    }

    fn events_of(&mut self, som: &str, activity: &str) -> Vec<ScriptEvent> {
        self.live().events.iter().filter(|e| e.som == som && e.activity == activity).cloned().collect()
    }

    /// End the event: write what is left, lay the form out if a capped relayout is owed, and
    /// report what the limits cut.
    fn finish(mut self) -> Result<Changes, String> {
        if self.relayouts_skipped {
            self.relayouts = 0;
            self.relayouts_skipped = false;
            self.relayout()?;
        }
        self.flush()?;
        if self.skipped > 0 {
            let why = if self.run.ran_away {
                "one ran too long".to_string()
            } else if self.runs >= MAX_SCRIPTS_PER_EVENT {
                format!("more than {MAX_SCRIPTS_PER_EVENT} scripts ran")
            } else {
                format!("the scripts took more than {} seconds", self.clock.budget_ms / 1000)
            };
            self.run.out.errors.push(format!("The form's scripts were stopped: {why}; {} more were skipped", self.skipped));
        }
        if self.dropped_output > 0 {
            self.run.out.errors.push(format!("The form's scripts produced too much output; {} more messages were left out", self.dropped_output));
        }
        Ok(self.changes)
    }
}

/// On open: initialize scripts, then calculations. Returns what they changed.
pub(crate) fn on_open(doc: &mut pdfcraft_cos::Document, tpl: &Template, run: &mut XfaRun) -> Result<Changes, String> {
    let mut r = Runner::new(doc, tpl, run, OPEN_BUDGET_MS);
    let events: Vec<ScriptEvent> = r.live().events.iter().filter(|e| e.activity == "initialize" || e.activity == "docReady").cloned().collect();
    for ev in &events {
        r.run(ev, "")?;
    }
    r.calculates()?;
    r.finish()
}

/// After fields `names` took new values: each one's change, exit and validate scripts, then
/// the calculations once (or only those, after a reset). A validate script that answers false,
/// or fails, shows its message; the value stays, as Acrobat marks the field invalid rather
/// than reverting it.
pub(crate) fn on_changes(doc: &mut pdfcraft_cos::Document, tpl: &Template, names: &[String], run: &mut XfaRun) -> Result<(), String> {
    let mut r = Runner::new(doc, tpl, run, OPEN_BUDGET_MS);
    for name in names {
        let som = r.by_som().iter().find(|(_, f)| *f == name).map(|(s, _)| s.clone());
        let Some(som) = som else { continue };
        let value = r.fields().get(name).map(|f| f.value.join("\n")).unwrap_or_default();
        for ev in r.events_of(&som, "change").into_iter().chain(r.events_of(&som, "exit")) {
            r.run(&ev, &value)?;
        }
        for ev in r.events_of(&som, "validate") {
            let Some(o) = r.run(&ev, &value)? else { continue };
            if (o.result_bool == Some(false) || o.error.is_some()) && !value.trim().is_empty() {
                r.alert(ev.message.clone().unwrap_or_else(|| format!("{name}: the value is not valid")));
            }
        }
    }
    r.calculates()?;
    r.finish().map(|_| ())
}

/// A button's click script (or any event, by SOM path and activity), then calculations.
pub(crate) fn on_event(doc: &mut pdfcraft_cos::Document, tpl: &Template, som: &str, activity: &str, run: &mut XfaRun) -> Result<(), String> {
    let mut r = Runner::new(doc, tpl, run, EVENT_BUDGET_MS);
    let events = r.events_of(som, activity);
    if events.is_empty() {
        return Err(format!("{som} has no {activity} script"));
    }
    for ev in events {
        r.run(&ev, "")?;
    }
    // The calculations that follow get a budget of their own.
    r.clock = Clock::start(OPEN_BUDGET_MS);
    r.calculates()?;
    r.finish().map(|_| ())
}

/// The SOM path of field `name` when PdfCraft generated it from an XFA template and the
/// template has a click script for it (a button with a native action has none).
pub(crate) fn clickable_som(doc: &pdfcraft_cos::Document, tpl: &Template, name: &str) -> Option<String> {
    let som = pdfcraft_xfa::fields_by_som(doc).into_iter().find(|(_, f)| f == name).map(|(s, _)| s)?;
    let live = pdfcraft_xfa::form_tree(tpl, pdfcraft_xfa::data_of(doc).as_ref(), &pdfcraft_xfa::overrides(doc));
    live.events.iter().any(|e| e.som == som && e.activity == "click").then_some(som)
}

/// The parsed template of a laid-out XFA form, kept for the document's life, when it has
/// scripts to run (`None` otherwise: nothing about the form is scripted).
pub(crate) fn template(doc: &pdfcraft_cos::Document) -> Option<Arc<Template>> {
    pdfcraft_xfa::template_of(doc).ok().filter(pdfcraft_xfa::has_scripts).map(Arc::new)
}

/// Push buttons PdfCraft generated for XFA click scripts carry no PDF action (the script is
/// XFA's, not Acrobat JavaScript): give them one that runs the script by the field's SOM path.
pub(crate) fn mark_script_buttons(doc: &pdfcraft_cos::Document, fields: &mut [pdfcraft_forms::Field]) {
    for f in fields.iter_mut() {
        if f.kind == pdfcraft_forms::FieldKind::PushButton
            && f.button.is_none()
            && std::iter::once(f.obj)
                .chain(f.widgets.iter().map(|w| w.obj))
                .any(|r| doc.get(r).as_dict().is_some_and(|d| d.contains(pdfcraft_xfa::CLICK_KEY)))
        {
            f.button = Some(pdfcraft_forms::af::ButtonAction::Script(String::new()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_names_match_indexed_som_paths() {
        assert!(som_matches("form[0].page1[0].qty[0]", "qty"));
        assert!(som_matches("form[0].page1[0].qty[0]", "page1.qty"));
        assert!(som_matches("form[0].page1[0].qty[0]", "xfa.form.form.page1.qty"));
        assert!(som_matches("form[0].page1[0].table[0].row[2].amount[0]", "table"));
        assert!(!som_matches("form[0].page1[0].qty[0]", "qt"));
        assert!(!som_matches("form[0].page1[0].price[0]", "qty"));
        assert_eq!(
            split_instance(&Template::default(), "form[0].table[0].row[2]").map(|(p, ps, n, i)| (p.len(), ps, n, i)),
            Some((2, "form[0].table[0]".into(), "row".into(), 2))
        );
    }
}
