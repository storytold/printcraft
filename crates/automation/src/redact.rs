//! Redaction tools: mark areas, text, patterns or whole pages for redaction, apply the marks
//! (removing what they cover for good) or clear them.

use pdfcraft_engine::{
    Edit, Hidden, NewAnnotation, REDACT_MAX_WORDS, REDACTION_CODE_SETS, RedactPattern, RedactionCodeSet, Shape, Style, find_pattern, rect_quad,
    redact_word_list,
};
use serde_json::{Value, json};

use crate::comments::{DEFAULT_AUTHOR, parse_color};
use crate::{Args, Automation, Result, ToolError, failed};

impl Automation {
    pub(crate) fn redact_mark(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let (id, n) = (doc.id, doc.info.pages.len());
        let pages: Vec<usize> = match a.opt_ints("pages")? {
            Some(_) => self.pages(a, "pages")?,
            None => (0..n).collect(),
        };
        let overlay = match (a.opt_str("overlay")?, a.opt_str("code_set")?) {
            (Some(_), Some(_)) => return Err(ToolError::InvalidArgs("pass overlay or code_set with codes, not both".into())),
            (Some(o), None) => o.to_string(),
            (None, Some(id)) => {
                let set = RedactionCodeSet::from_id(id).ok_or_else(|| {
                    let ids: Vec<_> = REDACTION_CODE_SETS.iter().map(|s| s.id).collect();
                    ToolError::InvalidArgs(format!("unknown code_set {id:?}; allowed: {}", ids.join(", ")))
                })?;
                let picked = a.strs("codes")?;
                let allowed = || format!("allowed in {}: {}", set.id, set.codes.join(", "));
                match set.overlay(&picked) {
                    Ok(o) if !o.is_empty() => o,
                    Ok(_) => return Err(ToolError::InvalidArgs(format!("codes is empty; {}", allowed()))),
                    Err(bad) => return Err(ToolError::InvalidArgs(format!("unknown code {bad:?}; {}", allowed()))),
                }
            }
            (None, None) if a.get("codes").is_some() => return Err(ToolError::InvalidArgs("codes needs code_set".into())),
            (None, None) => String::new(),
        };
        let author = a.opt_str("author")?.unwrap_or(DEFAULT_AUTHOR).to_string();
        let mut style = Style::default_for(&Shape::Redact { quads: Vec::new(), overlay: String::new(), look: Default::default() });
        if let Some(c) = a.opt_str("fill")? {
            style.fill = Some(parse_color(c)?);
        }
        // (page, quads) per mark.
        let mut marks: Vec<(usize, Vec<[f64; 8]>)> = Vec::new();
        let chosen = [
            a.get("rect").is_some(),
            a.get("find").is_some(),
            a.get("words").is_some(),
            a.get("pattern").is_some(),
            a.opt_bool("whole_pages")?.unwrap_or(false),
        ];
        if chosen.iter().filter(|c| **c).count() != 1 {
            return Err(ToolError::InvalidArgs("pass exactly one of rect (with page), find, words, pattern or whole_pages: true".into()));
        }
        // Per searched word or phrase: how many marks it made.
        let mut matched_words: Vec<(String, usize)> = Vec::new();
        if a.get("rect").is_some() {
            let page = self.page(a)?;
            let info = &self.doc(a)?.info.pages[page];
            let r: Vec<f64> = a.get("rect").and_then(Value::as_array).map(|x| x.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
            let r = <[f64; 4]>::try_from(r).map_err(|_| ToolError::InvalidArgs("rect must be 4 numbers".into()))?;
            let q = info.view_rect_to_quad([r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32]);
            marks.push((page, vec![q]));
        } else if a.opt_bool("whole_pages")?.unwrap_or(false) {
            let info = &self.doc(a)?.info;
            for &p in &pages {
                let c = info.pages[p].crop;
                marks.push((p, vec![rect_quad([c[0] as f64, c[1] as f64, c[2] as f64, c[3] as f64])]));
            }
        } else {
            let pattern = match a.opt_str("pattern")? {
                Some(p) => Some(RedactPattern::from_id(p).ok_or_else(|| ToolError::InvalidArgs(format!("unknown pattern {p:?}")))?),
                None => None,
            };
            let needles: Vec<String> = match (a.opt_str("find")?, a.get("words")) {
                (Some(f), _) => vec![f.to_owned()],
                (None, Some(_)) => {
                    let raw = a.strs("words")?;
                    if raw.len() > REDACT_MAX_WORDS {
                        return Err(ToolError::InvalidArgs(format!("words has {} entries; at most {REDACT_MAX_WORDS} are allowed", raw.len())));
                    }
                    let list = redact_word_list(&raw.join("\n"));
                    if list.is_empty() {
                        return Err(ToolError::InvalidArgs("words must hold at least one non-empty word or phrase".into()));
                    }
                    list
                }
                (None, None) => Vec::new(),
            };
            matched_words = needles.iter().map(|w| (w.clone(), 0)).collect();
            let texts = self.page_texts(id, &pages)?;
            let info = self.doc(a)?.info.clone();
            for (&p, text) in pages.iter().zip(&texts) {
                let hits: Vec<(Option<usize>, _)> = match pattern {
                    Some(pat) => text.find_with(|chars| find_pattern(pat, chars)).into_iter().map(|h| (None, h)).collect(),
                    None => needles.iter().enumerate().flat_map(|(i, w)| text.find(w).into_iter().map(move |h| (Some(i), h))).collect(),
                };
                for (word, h) in hits {
                    let quads: Vec<[f64; 8]> = text.line_rects(h).into_iter().map(|r| info.pages[p].view_rect_to_quad(r)).collect();
                    if !quads.is_empty() {
                        marks.push((p, quads));
                        if let Some(m) = word.and_then(|i| matched_words.get_mut(i)) {
                            m.1 += 1;
                        }
                    }
                }
            }
            if marks.is_empty() {
                return Err(failed("nothing matched, so nothing was marked"));
            }
        }
        let count = marks.len();
        let edits: Vec<Edit> = marks
            .iter()
            .map(|(page, quads)| {
                let shape = Shape::Redact { quads: quads.clone(), overlay: overlay.clone(), look: Default::default() };
                Edit::AddAnnotation(NewAnnotation { page: *page, shape, style: style.clone(), contents: String::new(), author: author.clone() })
            })
            .collect();
        let edit = match <[Edit; 1]>::try_from(edits) {
            Ok([one]) => one,
            Err(edits) => Edit::Batch { label: "Mark for redaction".into(), edits },
        };
        let mut out = self.apply(a, edit)?;
        out["marked"] = json!(count);
        out["pages_marked"] = json!(marks.iter().map(|m| m.0 + 1).collect::<std::collections::BTreeSet<_>>());
        out["marks_pending"] = json!(self.doc(a)?.redaction_marks());
        if a.get("words").is_some() {
            out["matched_words"] = json!(matched_words.iter().map(|(w, n)| json!({ "word": w, "marked": n })).collect::<Vec<_>>());
        }
        Ok(out)
    }

    pub(crate) fn redact_apply(&mut self, a: &Args) -> Result<Value> {
        let pages = match a.opt_ints("pages")? {
            Some(_) => Some(self.pages(a, "pages")?),
            None => None,
        };
        let before = self.doc(a)?.redaction_marks();
        if before == 0 {
            return Err(failed("there are no redaction marks to apply (see redact_mark)"));
        }
        let mut out = self.apply(a, Edit::ApplyRedactions { pages })?;
        let after = self.doc(a)?.redaction_marks();
        out["applied"] = json!(before - after);
        out["marks_pending"] = json!(after);
        Ok(out)
    }

    pub(crate) fn redact_clear(&mut self, a: &Args) -> Result<Value> {
        if self.doc(a)?.redaction_marks() == 0 {
            return Err(failed("there are no redaction marks"));
        }
        self.apply(a, Edit::ClearRedactions)
    }

    pub(crate) fn doc_hidden_info(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let items: Vec<Value> = doc.hidden_info().into_iter().map(|(h, n)| json!({ "category": h.id(), "label": h.label(), "count": n })).collect();
        let total: usize = doc.hidden_info().iter().map(|c| c.1).sum();
        Ok(json!({ "total": total, "categories": items }))
    }

    pub(crate) fn doc_audit_redactions(&self, a: &Args) -> Result<Value> {
        let findings = self.doc(a)?.audit_redactions().map_err(ToolError::Failed)?;
        let items: Vec<Value> = findings
            .into_iter()
            .map(|f| {
                json!({
                    "page": f.page + 1,
                    "kind": f.kind.id(),
                    "rect": f.rect,
                    "covered_text": f.covered_text,
                })
            })
            .collect();
        Ok(json!({ "findings": items, "count": items.len() }))
    }

    pub(crate) fn doc_remove_hidden(&mut self, a: &Args) -> Result<Value> {
        let edit = match a.get("categories") {
            None => Edit::Sanitize,
            Some(_) => {
                let which = a
                    .strs("categories")?
                    .into_iter()
                    .map(|c| Hidden::from_id(c).ok_or_else(|| ToolError::InvalidArgs(format!("unknown category {c:?} (see doc_hidden_info)"))))
                    .collect::<Result<Vec<_>>>()?;
                if which.is_empty() {
                    return Err(ToolError::InvalidArgs("categories is empty".into()));
                }
                Edit::RemoveHidden { which }
            }
        };
        let before: usize = self.doc(a)?.hidden_info().iter().map(|c| c.1).sum();
        let mut out = self.apply(a, edit)?;
        let after: usize = self.doc(a)?.hidden_info().iter().map(|c| c.1).sum();
        out["removed"] = json!(before.saturating_sub(after));
        Ok(out)
    }
}
