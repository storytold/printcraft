use std::sync::Arc;

use pdfcraft_cos::{Document, Object, SaveOptions, write_incremental};

use super::*;
use crate::data::{
    DatasetsWrite, FieldData, FieldDatum, build_data, iso_to_pattern, parse_datasets, pattern_to_iso, read_values, som_to_path, write_datasets,
};
use crate::data::{add_data_instances, remove_data_instance, write_data_value};
use crate::fixtures::{TINY_GIF, fields_template, scripted_template, shell, static_shell, template, template_with_data, tiny_png};
use crate::layout::Item;
use crate::layout::WidgetKind;
use crate::model::*;
use crate::script::{NodeKind, Overrides, apply_overrides, fields_by_som, form_tree, overrides, rerender, set_overrides};

fn widgets(page: &Page) -> Vec<&Widget> {
    page.items.iter().filter_map(|i| if let Item::Widget(w) = i { Some(w.as_ref()) } else { None }).collect()
}

fn texts(page: &Page) -> Vec<String> {
    page.items.iter().filter_map(|i| if let Item::Text(s) = i { Some(s.text.clone()) } else { None }).collect()
}

#[test]
fn measurements_convert_to_points() {
    assert_eq!(measure("72pt"), Some(72.0));
    assert_eq!(measure("1in"), Some(72.0));
    assert!((measure("25.4mm").unwrap() - 72.0).abs() < 1e-9);
    assert!((measure("2.54cm").unwrap() - 72.0).abs() < 1e-9);
    assert_eq!(measure(" 12 "), Some(12.0));
    assert_eq!(measure("3em"), None);
    assert_eq!(measure("abc"), None);
    assert_eq!(measure("NaNpt"), None);
}

#[test]
fn the_template_parses_with_fonts_captions_and_rich_text() {
    let (tpl, warnings) = parse(&template(2)).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(tpl.root.layout, Layout::Tb);
    let ps = tpl.root.page_set.as_ref().unwrap();
    assert_eq!(ps.areas.len(), 2);
    assert_eq!((ps.areas[0].width, ps.areas[0].height), (612.0, 792.0));
    assert_eq!(ps.areas[0].occur_max, Some(1));
    assert_eq!(ps.areas[1].occur_max, None);
    assert_eq!(ps.areas[0].content[0], Rect::new(36.0, 36.0, 540.0, 720.0));
    assert_eq!(tpl.page_roles, vec![("pageNo".to_string(), PageRole::Number), ("pageCount".to_string(), PageRole::Count)]);
    let Node::Subform(head) = &tpl.root.children[0] else { panic!() };
    assert_eq!(head.layout, Layout::LrTb);
    let Node::Draw(title) = &head.children[0] else { panic!() };
    let Value::Rich(r) = &title.value else { panic!("rich text") };
    assert_eq!(r.paragraphs.len(), 2, "{r:?}");
    assert_eq!(r.paragraphs[0].runs[0].bold, Some(true));
    assert_eq!(r.plain(), "Sample Form\nSecond line");
    let Node::Field(f) = &head.children[2] else { panic!() };
    assert_eq!(f.common.name.as_deref(), Some("familyName"));
    assert_eq!(f.common.font.typeface.as_deref(), Some("Courier New"));
    assert_eq!(f.max_chars, Some(30));
    assert_eq!(f.tooltip.as_deref(), Some("Your family name"));
    let cap = f.caption.as_ref().unwrap();
    assert_eq!((cap.placement, cap.reserve), (Placement::Top, Some(14.4)));
    // The widget border: three hidden edges and a visible bottom one.
    assert_eq!(f.ui_border.as_ref().unwrap().visible_edges(), [false, false, true, false]);
    let Node::Field(check) = &head.children[3] else { panic!() };
    assert_eq!((check.ui.clone(), check.items.clone()), (Ui::CheckButton, vec!["1".to_string(), "0".to_string()]));
    let Node::Field(date) = &head.children[4] else { panic!() };
    assert_eq!((date.ui.clone(), date.picture.as_deref()), (Ui::DateTimeEdit, Some("date{YYYY-MM-DD}")));
    let Node::ExclGroup(g) = &head.children[6] else { panic!() };
    assert_eq!(g.fields.len(), 2);
    let Node::Subform(table) = &tpl.root.children[1] else { panic!() };
    assert_eq!(table.column_widths, vec![144.0, 216.0, 180.0]);
    assert_eq!(table.overflow_leader.as_deref(), Some("header"));
    let Node::Subform(last) = &tpl.root.children[2] else { panic!() };
    assert!(last.break_before_page);
}

#[test]
fn the_xdp_template_wins_over_the_config_packets_template_element() {
    // The config packet has a <template> too; the real one is in the xfa-template namespace.
    let (tpl, _) = parse(&template(0)).unwrap();
    assert_eq!(tpl.root.common.name.as_deref(), Some("form"));
}

#[test]
fn layout_places_captions_fields_and_sizes_containers_to_their_content() {
    let form = layout_xml(&template(1)).unwrap();
    assert_eq!(form.pages.len(), 2, "head and table on page 1, the last section after its break");
    let p1 = &form.pages[0];
    let t = texts(p1);
    assert!(t.iter().any(|s| s == "Sample Form") && t.iter().any(|s| s == "Second line"), "{t:?}");
    assert!(t.iter().any(|s| s == "Family name"), "caption drawn: {t:?}");
    assert!(!t.iter().any(|s| s.contains("never shown")), "hidden draws take no space and draw nothing");
    assert!(t.iter().any(|s| s == "Page 1 of 2"), "embedded page number and count: {t:?}");
    assert!(!texts(&form.pages[1]).iter().any(|s| s.starts_with("Page ")), "the second master has no page-number draw");
    let w = widgets(p1);
    let family = w.iter().find(|w| w.name == "familyName").unwrap();
    // Below the 0.4in title, on a line of its own (3in × 0.5in); the caption takes the top 0.2in.
    assert!((family.rect.x - 36.0).abs() < 0.01 && (family.rect.y - 79.2).abs() < 0.01, "{:?}", family.rect);
    assert!((family.rect.w - 216.0).abs() < 0.01 && (family.rect.h - 21.6).abs() < 0.01, "{:?}", family.rect);
    assert_eq!(family.kind, WidgetKind::Text);
    assert_eq!(family.border.shape, BorderShape::Underline);
    assert_eq!(family.max_chars, Some(30));
    assert_eq!(family.tooltip.as_deref(), Some("Your family name"));
    assert_eq!(family.face.family, crate::text::Family::Courier);
    assert_eq!(family.som, "form[0].head[0].familyName[0]");
    let agree = w.iter().find(|w| w.name == "agree").unwrap();
    assert_eq!(agree.kind, WidgetKind::CheckBox { on: "1".into(), round: false });
    assert!((agree.rect.w - 10.0).abs() < 0.01, "a 10 pt square: {:?}", agree.rect);
    let born = w.iter().find(|w| w.name == "born").unwrap();
    assert_eq!(born.kind, WidgetKind::Date("yyyy-mm-dd".into()));
    // The radio group has no width: it sits beside the question on the same line.
    let q_y = p1
        .items
        .iter()
        .find_map(|i| {
            if let Item::Text(s) = i
                && s.text == "Have you ever?"
            {
                Some(s.baseline)
            } else {
                None
            }
        })
        .unwrap();
    let yes = w.iter().find(|w| matches!(&w.kind, WidgetKind::Radio { group, on, .. } if group == "answer" && on == "Y")).unwrap();
    assert!(yes.rect.x > 6.5 * 72.0 && yes.rect.y < q_y, "beside the question, not under it: {:?} vs baseline {q_y}", yes.rect);
    let go = w.iter().find(|w| w.name == "go").unwrap();
    assert_eq!(go.kind, WidgetKind::Button { caption: "Reset".into() });
    assert_eq!(go.action, Some(Action::Reset));
    assert_eq!(go.border.fill, Some([212.0 / 255.0, 208.0 / 255.0, 200.0 / 255.0]));
    // Table cells take the column widths; the second column has no width of its own.
    let what = w.iter().find(|w| w.name == "what0").unwrap();
    assert!((what.rect.x - (36.0 + 144.0)).abs() < 0.01 && (what.rect.w - 216.0).abs() < 0.01, "{:?}", what.rect);
    assert_eq!(
        form.fields,
        9,
        "familyName, agree, born, yes, no, go, 3 cells; pageNo/pageCount are hidden: {:?}",
        w.iter().map(|w| &w.name).collect::<Vec<_>>()
    );
}

#[test]
fn tables_break_across_pages_and_repeat_their_header_row() {
    let form = layout_xml(&template(40)).unwrap();
    assert!(form.pages.len() >= 3, "{} pages", form.pages.len());
    // The header row's "Activity" cell appears at the top of every page the table continues on.
    for (i, p) in form.pages.iter().enumerate().take(form.pages.len() - 1) {
        let headers = p.items.iter().filter(|it| matches!(it, Item::Text(s) if s.text == "Activity")).count();
        assert_eq!(headers, 1, "page {}: header rows {headers}", i + 1);
    }
    // Rows are whole: no cell straddles the bottom of the content area.
    for p in &form.pages {
        for w in widgets(p) {
            assert!(w.rect.bottom() <= 36.0 + 720.0 + 0.01, "{} ends below the content area: {:?}", w.name, w.rect);
        }
    }
    let all: Vec<&str> = form.pages.iter().flat_map(|p| widgets(p).into_iter().map(|w| w.name.as_str()).collect::<Vec<_>>()).collect();
    assert_eq!(all.iter().filter(|n| n.starts_with("from")).count(), 40);
    assert_eq!(all.len(), all.iter().collect::<std::collections::HashSet<_>>().len(), "field names are unique");
}

#[test]
fn rendering_into_the_pdf_replaces_the_placeholder_page_and_adds_fields() {
    let mut doc = Document::open(Arc::new(shell(&template(2)))).unwrap();
    assert!(is_dynamic(&doc));
    let report = render_into(&mut doc).unwrap();
    assert_eq!((report.pages, report.fields), (2, 11), "fields: 5 on the head, the radio group once, 6 cells");
    let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let doc = Document::open(Arc::new(bytes)).unwrap();
    let root = doc.get(doc.root().unwrap());
    let catalog = root.as_dict().unwrap();
    let pages = doc.resolve(catalog.get(b"Pages").unwrap());
    assert_eq!(pages.as_dict().unwrap().int(b"Count"), Some(2));
    let kids = doc.resolve(pages.as_dict().unwrap().get(b"Kids").unwrap()).as_array().unwrap().clone();
    let page1 = doc.resolve(&kids[0]);
    let annots = doc.resolve(page1.as_dict().unwrap().get(b"Annots").unwrap()).as_array().unwrap().len();
    assert!(annots >= 8, "{annots} widgets on page 1");
    let acro = doc.resolve(catalog.get(b"AcroForm").unwrap());
    let acro = acro.as_dict().unwrap();
    assert!(acro.contains(b"XFA"), "the XFA packets stay");
    assert_eq!(catalog.get(b"NeedsRendering"), Some(&Object::Bool(true)), "still a dynamic form for Adobe's viewers");
    let fields = doc.resolve(acro.get(b"Fields").unwrap()).as_array().unwrap().len();
    assert_eq!(fields, 11, "top-level fields (the radio group counts once)");
    let dr = doc.resolve(acro.get(b"DR").unwrap());
    let fonts = doc.resolve(dr.as_dict().unwrap().get(b"Font").unwrap());
    assert!(fonts.as_dict().unwrap().contains(b"Cour") && fonts.as_dict().unwrap().contains(b"ZaDb"));
    // A field carries its SOM path for data round trips.
    let first = doc.resolve(&doc.resolve(acro.get(b"Fields").unwrap()).as_array().unwrap()[0]);
    assert!(first.as_dict().unwrap().contains(SOM_KEY));
    // Not dynamic twice: a second pass is refused rather than doubling the pages.
    assert!(!is_dynamic(&doc));
}

#[test]
fn documents_without_xfa_are_not_touched() {
    let mut doc = Document::new_empty();
    assert!(!is_dynamic(&doc));
    assert!(matches!(render_into(&mut doc), Err(XfaError::NotXfa)));
}

#[test]
fn hostile_templates_fail_without_panicking() {
    assert!(layout_xml("not xml").is_err());
    assert!(layout_xml("<template/>").is_err());
    assert!(layout_xml("<template><subform/></template>").is_ok(), "an empty form is one empty page");
    // Deep nesting is cut off, not followed.
    let deep = format!("<template><subform layout=\"tb\">{}{}</subform></template>", "<subform layout=\"tb\">".repeat(500), "</subform>".repeat(500));
    assert!(layout_xml(&deep).is_ok());
    // Absurd sizes are clamped; a page can't be a mile wide.
    let huge = r##"<template><subform layout="tb"><pageSet><pageArea><medium short="1e9in" long="-5in"/></pageArea></pageSet><draw w="1e12pt" h="1e12pt"><value><text>x</text></value></draw></subform></template>"##;
    let f = layout_xml(huge).unwrap();
    assert!(f.pages[0].width <= 14_400.0 && f.pages[0].height >= 1.0);
    // Too many pages is an error, not an endless loop.
    let many = format!(
        "<template><subform layout=\"tb\"><pageSet><pageArea><contentArea w=\"100pt\" h=\"100pt\"/><medium short=\"100pt\" long=\"100pt\"/></pageArea></pageSet>{}</subform></template>",
        "<draw h=\"90pt\"><value><text>x</text></value></draw>".repeat(MAX_PAGES + 5)
    );
    assert!(matches!(layout_xml(&many), Err(XfaError::TooLarge(_))));
    // A bad image and a bad base64 are warnings, not failures.
    let img = r##"<template><subform layout="tb"><draw w="1in" h="1in"><value><image contentType="image/png">!!!notbase64</image></value></draw></subform></template>"##;
    let f = layout_xml(img).unwrap();
    assert!(!f.warnings.is_empty());
}

#[test]
fn the_packets_can_be_one_stream_or_named_parts() {
    // Named parts, with the template split from the rest.
    let doc = Document::open(Arc::new(shell(&template(0)))).unwrap();
    let p = read_packets(&doc).unwrap().unwrap();
    assert!(p.needs_rendering && !p.has_fields);
    assert!(p.xdp.contains("<template"));
    // UTF-16 packets are read too.
    let mut utf16 = vec![0xFF, 0xFE];
    for u in "<template><subform/></template>".encode_utf16() {
        utf16.extend_from_slice(&u.to_le_bytes());
    }
    let xdp = String::from_utf8_lossy(&utf16).into_owned();
    let _ = xdp;
    let doc = Document::open(Arc::new(shell_bytes(&utf16))).unwrap();
    let p = read_packets(&doc).unwrap().unwrap();
    assert!(p.xdp.starts_with("<template>"), "{:?}", &p.xdp[..20.min(p.xdp.len())]);
}

/// Like [`shell`], but the packet bytes are given as-is.
fn shell_bytes(packet: &[u8]) -> Vec<u8> {
    let text = shell("PLACEHOLDER");
    let needle = b"<< /Length 11 >>\nstream\nPLACEHOLDER\nendstream";
    let pos = text.windows(needle.len()).position(|w| w == needle).unwrap();
    let mut out = text[..pos].to_vec();
    out.extend_from_slice(format!("<< /Length {} >>\nstream\n", packet.len()).as_bytes());
    out.extend_from_slice(packet);
    out.extend_from_slice(b"\nendstream");
    out.extend_from_slice(&text[pos + needle.len()..]);
    // The xref offsets after the stream are off; the reader reconstructs them.
    out
}

#[test]
fn jpeg_headers_give_the_size() {
    // A minimal SOF0 header: 300 × 200, 3 components.
    let mut j = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    j.extend_from_slice(&[0; 14]);
    j.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0xC8, 0x01, 0x2C, 0x03]);
    assert_eq!(pdf::jpeg_size(&j), Some((300, 200)));
    assert_eq!(pdf::jpeg_size(b"\x89PNG"), None);
    assert_eq!(pdf::jpeg_size(&[0xFF, 0xD8, 0xFF]), None);
}

const DATA: &str = "<form><head><familyName>Singh</familyName><agree>1</agree><born>2001-02-03</born><answer>N</answer></head><table><row><from0>2019</from0><what0>Studied</what0><where0>Delhi</where0></row><row><from0>2021</from0><what0>Worked</what0><where0>Toronto</where0></row><row><from0>2023</from0><what0>Moved</what0><where0>Ottawa</where0></row></table></form>";

#[test]
fn datasets_parse_into_a_tree_and_som_paths_find_their_nodes() {
    let d = parse_datasets(&template_with_data(1, DATA)).expect("datasets");
    assert_eq!(d.name, "data");
    assert_eq!(d.text_at(&som_to_path("form[0].head[0].familyName[0]")), Some("Singh"));
    assert_eq!(d.text_at(&som_to_path("form[0].#subform[0].head[0].answer[0]")), Some("N"), "unnamed containers have no data node");
    assert_eq!(d.text_at(&som_to_path("form[0].table[0].row[2].where0[0]")), Some("Ottawa"));
    assert_eq!(d.text_at(&som_to_path("form[0].table[0].row[3].where0[0]")), None);
    assert_eq!(d.count(&som_to_path("form[0].table[0]"), "row"), 3);
    assert_eq!(som_to_path("a[1].b"), vec![("a".to_string(), 1), ("b".to_string(), 0)]);
    assert!(parse_datasets("<xdp:xdp xmlns:xdp=\"x\"/>").is_none());
}

#[test]
fn dates_convert_between_iso_and_acrobat_patterns() {
    assert_eq!(iso_to_pattern("2001-02-03", "yyyy-mm-dd"), "2001-02-03");
    assert_eq!(iso_to_pattern("2001-02-03", "mm/dd/yyyy"), "02/03/2001");
    assert_eq!(iso_to_pattern("2001-02-03", "d.m.yy"), "3.2.01");
    assert_eq!(iso_to_pattern("not a date", "mm/dd/yyyy"), "not a date");
    assert_eq!(pattern_to_iso("02/03/2001", "mm/dd/yyyy").as_deref(), Some("2001-02-03"));
    assert_eq!(pattern_to_iso("3.2.01", "d.m.yy").as_deref(), Some("2001-02-03"));
    assert_eq!(pattern_to_iso("2001-02-03", "yyyy-mm-dd").as_deref(), Some("2001-02-03"));
    assert_eq!(pattern_to_iso("13/40/2001", "mm/dd/yyyy"), None);
    assert_eq!(pattern_to_iso("hello", "mm/dd/yyyy"), None);
}

#[test]
fn data_fills_the_fields_and_adds_rows() {
    let form = layout_xml(&template_with_data(1, DATA)).unwrap();
    let all: Vec<&Widget> = form.pages.iter().flat_map(widgets).collect();
    let by = |n: &str| all.iter().find(|w| w.name == n).unwrap_or_else(|| panic!("no widget {n}"));
    assert_eq!(by("familyName").value.as_deref(), Some("Singh"));
    assert_eq!(by("agree").value.as_deref(), Some("1"), "checked by its on value");
    assert_eq!(by("born").value.as_deref(), Some("2001-02-03"));
    let no = all.iter().find(|w| matches!(&w.kind, WidgetKind::Radio { on, .. } if on == "N")).unwrap();
    let yes = all.iter().find(|w| matches!(&w.kind, WidgetKind::Radio { on, .. } if on == "Y")).unwrap();
    assert!(no.value.is_some() && yes.value.is_none(), "the group's data picks the N button");
    // One row in the template, three in the data: three rows laid out, each with its values.
    let wheres: Vec<&str> = all.iter().filter(|w| w.name.starts_with("where0")).filter_map(|w| w.value.as_deref()).collect();
    assert_eq!(wheres, ["Delhi", "Toronto", "Ottawa"]);
    assert_eq!(by("where0_3").som, "form[0].table[0].row[2].where0[0]");
    assert_eq!(by("agree").items, vec!["1".to_string(), "0".to_string()]);
}

/// The AcroForm's terminal fields as the engine would hand them over.
fn field_data(doc: &Document) -> Vec<FieldDatum> {
    let root = doc.get(doc.root().unwrap());
    let acro = doc.resolve(root.as_dict().unwrap().get(b"AcroForm").unwrap());
    let fields = doc.resolve(acro.as_dict().unwrap().get(b"Fields").unwrap()).as_array().unwrap().clone();
    let mut out = Vec::new();
    for f in fields {
        let r = f.as_ref().unwrap();
        let d = doc.get(r);
        let d = d.as_dict().unwrap();
        let name = d.get(b"T").and_then(|t| t.as_string()).map(|s| s.to_text()).unwrap_or_default();
        let v = d.get(b"V").map(|v| doc.resolve(v));
        let data = match d.name(b"FT") {
            Some(b"Tx") => FieldData::Text(v.and_then(|v| v.as_string().map(|s| s.to_text())).unwrap_or_default()),
            Some(b"Btn") if d.int(b"Ff").unwrap_or(0) & (1 << 15) != 0 => {
                FieldData::Radio(v.and_then(|v| v.as_name().map(|n| String::from_utf8_lossy(n).into_owned())).filter(|n| n != "Off"))
            }
            Some(b"Btn") if d.int(b"Ff").unwrap_or(0) & (1 << 16) != 0 => FieldData::None,
            Some(b"Btn") => FieldData::Check(v.and_then(|v| v.as_name().map(|n| n != b"Off")).unwrap_or(false)),
            _ => FieldData::None,
        };
        out.push(FieldDatum { obj: r, name, data });
    }
    out
}

#[test]
fn the_datasets_packet_is_written_from_the_fields_and_read_back() {
    let mut doc = Document::open(Arc::new(shell(&template_with_data(1, DATA)))).unwrap();
    render_into(&mut doc).unwrap();
    // The laid-out fields hold the data's values; writing them back reproduces it.
    let fields = field_data(&doc);
    assert!(fields.iter().any(|f| f.name == "familyName" && f.data == FieldData::Text("Singh".into())), "{fields:?}");
    assert!(fields.iter().any(|f| f.name == "answer" && f.data == FieldData::Radio(Some("N".into()))), "{fields:?}");
    // The data already holds what the fields hold: nothing to write.
    assert_eq!(write_datasets(&mut doc, &fields).unwrap(), DatasetsWrite::default());
    let doc = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let p = read_packets(&doc).unwrap().unwrap();
    let d = parse_datasets(&p.xdp).unwrap();
    assert_eq!(d.text_at(&som_to_path("form[0].head[0].familyName[0]")), Some("Singh"));
    assert_eq!(d.text_at(&som_to_path("form[0].head[0].agree[0]")), Some("1"));
    assert_eq!(d.text_at(&som_to_path("form[0].head[0].answer[0]")), Some("N"));
    assert_eq!(d.text_at(&som_to_path("form[0].table[0].row[2].where0[0]")), Some("Ottawa"));
    assert_eq!(p.xdp.matches("<xfa:datasets").count(), 1, "the packet is replaced, not duplicated");
    // Change a value, write again: the data follows, and reading gives the field its value.
    let mut doc = doc;
    let mut fields = field_data(&doc);
    for f in &mut fields {
        if f.name == "familyName" {
            f.data = FieldData::Text("Kaur".into());
        }
        if f.name == "agree" {
            f.data = FieldData::Check(false);
        }
    }
    let w = write_datasets(&mut doc, &fields).unwrap();
    assert!(w.written && w.warnings.is_empty(), "{w:?}");
    let values = read_values(&doc, &fields);
    assert!(values.contains(&("familyName".to_string(), FieldData::Text("Kaur".into()))), "{values:?}");
    assert!(values.contains(&("agree".to_string(), FieldData::Check(false))), "{values:?}");
    // A form without a datasets packet gets one, before the postamble.
    let mut doc = Document::open(Arc::new(shell(&template(1)))).unwrap();
    render_into(&mut doc).unwrap();
    let fields = field_data(&doc);
    assert!(write_datasets(&mut doc, &fields).unwrap().written);
    let p = read_packets(&doc).unwrap().unwrap();
    assert!(parse_datasets(&p.xdp).is_some());
    // No XFA at all: nothing to write.
    let mut plain = Document::new_empty();
    assert!(!write_datasets(&mut plain, &[]).unwrap().written);
}

#[test]
fn static_forms_read_their_values_from_the_datasets() {
    let doc = Document::open(Arc::new(static_shell("<form1><page1><name>Ada</name><agree>1</agree></page1></form1>"))).unwrap();
    assert!(!is_dynamic(&doc));
    let fields = vec![
        FieldDatum { obj: pdfcraft_cos::ObjRef::new(8, 0), name: "form1[0].page1[0].name[0]".into(), data: FieldData::Text(String::new()) },
        FieldDatum { obj: pdfcraft_cos::ObjRef::new(9, 0), name: "form1[0].page1[0].agree[0]".into(), data: FieldData::Check(false) },
    ];
    let values = read_values(&doc, &fields);
    assert_eq!(
        values,
        vec![
            ("form1[0].page1[0].name[0]".to_string(), FieldData::Text("Ada".into())),
            ("form1[0].page1[0].agree[0]".to_string(), FieldData::Check(true))
        ]
    );
    assert_eq!(build_data(&doc, &fields), "<xfa:data><form1><page1><name></name><agree></agree></page1></form1></xfa:data>");
}

#[test]
fn nested_width_less_subforms_lay_out_in_polynomial_time() {
    // Regression: a container without a width measured its content once for its width and
    // again for its height, at every level: 2^depth measurements. Two children per level make
    // it worse still. This must finish at once, and lay the innermost fields out.
    for depth in [40, 60] {
        let open = "<subform layout=\"lr-tb\"><field name=\"a\" w=\"20pt\" h=\"10pt\"/>".repeat(depth);
        let xml = format!(
            "<template><subform layout=\"tb\" name=\"form\">{open}<field name=\"inner\" w=\"30pt\" h=\"10pt\"/>{}</subform></template>",
            "</subform>".repeat(depth)
        );
        let start = std::time::Instant::now();
        let form = layout_xml(&xml).expect("lays out");
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "took {:?}", start.elapsed());
        let names: Vec<String> = form.pages.iter().flat_map(widgets).map(|w| w.name.clone()).collect();
        assert!(names.iter().any(|n| n == "inner") || depth > 60, "{depth}: {names:?}");
        assert!(names.len() > 1);
    }
}

/// The static fixture's two fields, holding `name` and `agree`.
fn static_fields(name: &str, agree: bool) -> Vec<FieldDatum> {
    vec![
        FieldDatum { obj: pdfcraft_cos::ObjRef::new(8, 0), name: "form1[0].page1[0].name[0]".into(), data: FieldData::Text(name.into()) },
        FieldDatum { obj: pdfcraft_cos::ObjRef::new(9, 0), name: "form1[0].page1[0].agree[0]".into(), data: FieldData::Check(agree) },
    ]
}

/// The decoded XFA stream (object 5 in the static fixture, or whatever `/XFA` now points to).
fn xfa_bytes(doc: &Document) -> Vec<u8> {
    let root = doc.get(doc.root().unwrap());
    let acro = doc.resolve(root.as_dict().unwrap().get(b"AcroForm").unwrap());
    let xfa = doc.resolve(acro.as_dict().unwrap().get(b"XFA").unwrap());
    let Object::Stream(s) = &*xfa else { panic!("a stream") };
    s.decoded_within(1 << 20).unwrap()
}

#[test]
fn writing_values_keeps_the_data_no_field_binds_to() {
    // Regression: the data element was rebuilt from the fields alone, dropping everything else.
    let data = concat!(
        "<form1 a=\"1\"><page1><name>Ada</name><notes xmlns:my=\"urn:my\"><my:extra kind='x>y'>keep me</my:extra></notes>",
        "<rich><p>bold <b>text</b></p></rich></page1><deep><a><b><c>deep value</c></b></a></deep></form1>",
        "<other xmlns=\"urn:other\"><x>unbound</x></other><!-- a comment --><empty/>"
    );
    let mut doc = Document::open(Arc::new(static_shell(data))).unwrap();
    let w = write_datasets(&mut doc, &static_fields("Grace & <Co>\r\n", true)).unwrap();
    assert!(w.written && w.warnings.is_empty(), "{w:?}");
    let saved = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let xdp = String::from_utf8(xfa_bytes(&saved)).unwrap();
    // Everything no field binds to is still there, as written.
    for kept in [
        "<form1 a=\"1\">",
        "<notes xmlns:my=\"urn:my\"><my:extra kind='x>y'>keep me</my:extra></notes>",
        "<rich><p>bold <b>text</b></p></rich>",
        "<deep><a><b><c>deep value</c></b></a></deep>",
        "<other xmlns=\"urn:other\"><x>unbound</x></other><!-- a comment --><empty/>",
        "<template xmlns=",
    ] {
        assert!(xdp.contains(kept), "lost {kept:?}: {xdp}");
    }
    // The bound value changed in place; the missing check box node was added beside it.
    assert!(xdp.contains("<name>Grace &amp; &lt;Co&gt;&#xD;\n</name>"), "{xdp}");
    let d = parse_datasets(&xdp).unwrap();
    assert_eq!(d.text_at(&som_to_path("form1[0].page1[0].name[0]")), Some("Grace & <Co>\r\n"));
    assert_eq!(d.text_at(&som_to_path("form1[0].page1[0].agree[0]")), Some("1"));
    assert_eq!(xdp.matches("<page1>").count(), 1, "added into the existing node, not a second one");
    // Writing the same values again changes nothing.
    let mut again = saved;
    assert_eq!(write_datasets(&mut again, &static_fields("Grace & <Co>\r\n", true)).unwrap(), DatasetsWrite::default());
    // A field bound to a node holding structured content is reported, not flattened.
    let mut doc = Document::open(Arc::new(static_shell("<form1><page1><name><p>rich</p></name></page1></form1>"))).unwrap();
    let w = write_datasets(&mut doc, &static_fields("plain", false)).unwrap();
    assert!(w.warnings.iter().any(|m| m.contains("structured content") && m.contains("name[0]")), "{w:?}");
    assert!(String::from_utf8(xfa_bytes(&doc)).unwrap().contains("<name><p>rich</p></name>"));
}

#[test]
fn utf16_packets_are_read_and_written_back_as_utf16() {
    let mut doc = Document::open(Arc::new(static_shell("<form1><page1><name>Zoë 日本</name><other>ünbound</other></page1></form1>"))).unwrap();
    let text = String::from_utf8(xfa_bytes(&doc)).unwrap();
    for (encoding, bom) in [(Encoding::Utf16Le { bom: true }, &[0xFF, 0xFE][..]), (Encoding::Utf16Be { bom: true }, &[0xFE, 0xFF][..])] {
        let mut doc16 = doc.clone();
        doc16.set(
            pdfcraft_cos::ObjRef::new(5, 0),
            Object::Stream(pdfcraft_cos::Stream::flate(pdfcraft_cos::Dict::new(), &encode_packet(&text, encoding))),
        );
        // Read exactly, not as mojibake.
        let values = read_values(&doc16, &static_fields("", false));
        assert_eq!(values.first(), Some(&("form1[0].page1[0].name[0]".to_string(), FieldData::Text("Zoë 日本".into()))));
        let w = write_datasets(&mut doc16, &static_fields("Łódź", false)).unwrap();
        assert!(w.written && w.warnings.is_empty(), "{w:?}");
        let bytes = xfa_bytes(&doc16);
        assert!(bytes.starts_with(bom), "still UTF-16 with its byte order mark");
        let (back, enc) = decode_packet(&bytes).unwrap();
        assert_eq!(enc, encoding);
        assert!(back.contains("<name>Łódź</name>") && back.contains("<other>ünbound</other>"), "{back}");
    }
    // A packet in neither UTF-8 nor UTF-16 (Latin-1 here) is left alone, and that is reported.
    let latin1: Vec<u8> = text.replace("Zoë 日本", "Zo\u{eb}").chars().map(|c| c as u32 as u8).collect();
    doc.set(pdfcraft_cos::ObjRef::new(5, 0), Object::Stream(pdfcraft_cos::Stream::flate(pdfcraft_cos::Dict::new(), &latin1)));
    let w = write_datasets(&mut doc, &static_fields("x", true)).unwrap();
    assert!(!w.written && !w.warnings.is_empty(), "{w:?}");
    assert_eq!(xfa_bytes(&doc), latin1);
}

#[test]
fn huge_som_indices_are_capped_not_built() {
    // Regression: each index created its missing siblings one count at a time (quadratic), with
    // no limit on the total. Many fields with absurd indices must finish at once, reporting
    // what they could not write.
    let mut doc = Document::open(Arc::new(static_shell("<form1/>"))).unwrap();
    let start = std::time::Instant::now();
    let fields: Vec<FieldDatum> = (0..300)
        .map(|i| FieldDatum {
            obj: pdfcraft_cos::ObjRef::new(1000 + i, 0),
            name: format!("form1[0].a[{}].b[99999].c[99999].d[{i}]", 9_000 + i),
            data: FieldData::Text("v".into()),
        })
        .collect();
    let w = write_datasets(&mut doc, &fields).unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(10), "took {:?}", start.elapsed());
    assert!(w.warnings.iter().any(|m| m.contains("repeats too often")), "{w:?}");
    let xdp = String::from_utf8(xfa_bytes(&doc)).unwrap();
    assert!(xdp.matches("<a>").count() <= 20_000, "at most the node budget is created");
    // Building fresh data is capped the same way.
    let start = std::time::Instant::now();
    let data = build_data(&doc, &fields);
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
    assert!(data.len() < 2_000_000);
    // And a moderate index still works.
    let mut doc = Document::open(Arc::new(static_shell("<form1><row><v>1</v></row></form1>"))).unwrap();
    let row = |i: usize| FieldDatum {
        obj: pdfcraft_cos::ObjRef::new(500, 0),
        name: format!("form1[0].row[{i}].v[0]"),
        data: FieldData::Text(i.to_string()),
    };
    let w = write_datasets(&mut doc, &[row(0), row(3)]).unwrap();
    assert!(w.written && w.warnings.is_empty(), "{w:?}");
    let d = parse_datasets(&String::from_utf8(xfa_bytes(&doc)).unwrap()).unwrap();
    assert_eq!(d.count(&som_to_path("form1[0]"), "row"), 4);
    assert_eq!(d.text_at(&som_to_path("form1[0].row[3].v[0]")), Some("3"));
    assert!(d.get(&som_to_path("form1[0].row[2]")).is_some(), "the instances in between exist, empty");
}

#[test]
fn the_form_tree_carries_values_presence_instances_and_the_events() {
    let xml = scripted_template();
    let (tpl, _) = parse(&xml).unwrap();
    let crate::script::LiveForm { root, events, truncated } = form_tree(&tpl, None, &Overrides::default());
    assert!(!truncated);
    assert_eq!((root.name.as_str(), root.som.as_str()), ("form", "form[0]"));
    let page = &root.children[0];
    assert_eq!(page.som, "form[0].page1[0]");
    let by = |n: &str| page.children.iter().find(|c| c.name == n).unwrap_or_else(|| panic!("no {n}"));
    assert_eq!((by("price").value.as_str(), by("price").numeric, by("total").access.as_str()), ("5", true, "readOnly"));
    assert_eq!(by("details").presence, "hidden");
    let table = by("table");
    assert_eq!(table.children.len(), 1, "one row without data");
    assert!(table.children[0].repeatable && table.children[0].occur_max.is_none());
    assert_eq!(table.children[0].children[1].som, "form[0].page1[0].table[0].row[0].amount[0]");
    let ev = |som: &str, act: &str| events.iter().find(|e| e.som == som && e.activity == act).unwrap_or_else(|| panic!("no {act} on {som}"));
    assert!(ev("form[0].page1[0]", "initialize").script.contains("qty.rawValue = 2"));
    assert!(ev("form[0].page1[0].total[0]", "calculate").script.contains("qty.rawValue * price.rawValue"));
    assert_eq!(ev("form[0].page1[0].qty[0]", "validate").message.as_deref(), Some("Quantity must be 100 or less"));
    assert!(!ev("form[0].page1[0].addRow[0]", "click").formcalc);
    assert!(ev("form[0].page1[0].legacy[0]", "click").formcalc, "no contentType means FormCalc");
    // Data adds rows and values; overrides change presence.
    let data = parse_datasets("<xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data><form><page1><qty>7</qty><table><row><amount>1</amount></row><row><amount>2</amount></row><row><amount>3</amount></row></table></page1></form></xfa:data></xfa:datasets>").unwrap();
    let mut ov = Overrides::default();
    ov.presence.insert("form[0].page1[0].details[0]".into(), "visible".into());
    let root = form_tree(&tpl, Some(&data), &ov).root;
    let page = &root.children[0];
    assert_eq!(page.children.iter().find(|c| c.name == "qty").unwrap().value, "7");
    assert_eq!(page.children.iter().find(|c| c.name == "table").unwrap().children.len(), 3);
    assert_eq!(page.children.iter().find(|c| c.name == "details").unwrap().presence, "visible");
    assert_eq!(page.children.iter().find(|c| c.name == "details").unwrap().kind, Some(NodeKind::Subform));
}

#[test]
fn overrides_live_in_the_acroform_and_hide_objects_in_the_layout() {
    let mut doc = Document::open(Arc::new(shell(&scripted_template()))).unwrap();
    render_into(&mut doc).unwrap();
    assert!(overrides(&doc).is_empty());
    let names = |doc: &Document| fields_by_som(doc).into_values().collect::<Vec<_>>();
    assert!(!names(&doc).iter().any(|n| n == "note"), "details is hidden: {:?}", names(&doc));
    let mut ov = Overrides::default();
    ov.presence.insert("form[0].page1[0].details[0]".into(), "visible".into());
    ov.access.insert("form[0].page1[0].qty[0]".into(), "readOnly".into());
    set_overrides(&mut doc, &ov).unwrap();
    assert_eq!(overrides(&doc), ov);
    let (tpl, _) = parse(&scripted_template()).unwrap();
    let laid = apply_overrides(&tpl, &ov);
    let Node::Subform(page) = &laid.root.children[0] else { panic!() };
    let Node::Subform(details) = page.children.iter().find(|c| c.common().name.as_deref() == Some("details")).unwrap() else { panic!() };
    assert_eq!(details.common.presence, Presence::Visible);
    let Node::Field(qty) = page.children.iter().find(|c| c.common().name.as_deref() == Some("qty")).unwrap() else { panic!() };
    assert_eq!(qty.access, Access::ReadOnly);
    // Laying out again shows the note field and keeps the others.
    let report = rerender(&mut doc, &tpl).unwrap();
    let after = names(&doc);
    assert!(after.iter().any(|n| n == "note"), "{after:?}");
    assert!(after.iter().any(|n| n == "qty"));
    assert_eq!(report.fields, after.len());
    // Reopening the written bytes finds one set of fields, not two.
    let doc = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
    assert_eq!(names(&doc).len(), after.len());
    assert_eq!(overrides(&doc), ov);
    // Clearing removes the key.
    let mut doc = doc;
    set_overrides(&mut doc, &Overrides::default()).unwrap();
    assert!(overrides(&doc).is_empty());
}

#[test]
fn data_instances_are_added_and_removed_in_the_packet() {
    let mut doc = Document::open(Arc::new(shell(&scripted_template()))).unwrap();
    render_into(&mut doc).unwrap();
    let parent = som_to_path("form[0].page1[0].table[0]");
    // No datasets packet yet: one is created with the rows.
    assert!(add_data_instances(&mut doc, &parent, "row", 3).unwrap().written);
    let data = crate::script::data_of(&doc).unwrap();
    assert_eq!(data.count(&parent, "row"), 3);
    // Asking for fewer changes nothing; more adds.
    assert!(!add_data_instances(&mut doc, &parent, "row", 2).unwrap().written);
    assert!(add_data_instances(&mut doc, &parent, "row", 4).unwrap().written);
    let mut p = parent.clone();
    p.push(("row".into(), 1));
    p.push(("amount".into(), 0));
    write_data_value(&mut doc, &p, "42").unwrap();
    let data = crate::script::data_of(&doc).unwrap();
    assert_eq!((data.count(&parent, "row"), data.text_at(&p)), (4, Some("42")));
    // Remove the second row: the third's value moves up.
    let mut second = parent.clone();
    second.push(("row".into(), 1));
    assert!(remove_data_instance(&mut doc, &second).unwrap().written);
    let data = crate::script::data_of(&doc).unwrap();
    assert_eq!(data.count(&parent, "row"), 3);
    assert_eq!(data.text_at(&p), None, "the row holding 42 is gone");
    // Removing what isn't there is nothing.
    let mut tenth = parent.clone();
    tenth.push(("row".into(), 9));
    assert!(!remove_data_instance(&mut doc, &tenth).unwrap().written);
    // The layout follows the data: three rows of fields.
    let (tpl, _) = parse(&scripted_template()).unwrap();
    rerender(&mut doc, &tpl).unwrap();
    let names: Vec<String> = fields_by_som(&doc).into_values().collect();
    assert_eq!(names.iter().filter(|n| n.starts_with("amount")).count(), 3, "{names:?}");
    // Hostile: absurd indices are capped, not built.
    assert!(add_data_instances(&mut doc, &parent, "row", 1_000_000).is_ok());
}

#[test]
fn formcalc_button_idioms_keep_their_native_actions() {
    let xml = scripted_template()
        .replace("<script>$host.messageBox(\"FormCalc\")</script>", "<script>$host.resetData()</script>")
        .replace("xfa.host.messageBox(\"Hello \" + qty.rawValue);", "if (qty.rawValue > 1) { xfa.host.resetData(); }");
    let form = layout_xml(&xml).unwrap();
    let w: Vec<&Widget> = form.pages.iter().flat_map(widgets).collect();
    let by = |n: &str| w.iter().find(|w| w.name == n).unwrap_or_else(|| panic!("no {n}"));
    assert_eq!(by("legacy").action, Some(Action::Reset), "a one-statement FormCalc idiom is a native action");
    assert!(matches!(by("hello").action, Some(Action::Script(_))), "anything else runs as a script: {:?}", by("hello").action);
    assert!(matches!(by("addRow").action, Some(Action::Script(_))));
}

/// Five levels of subforms that each repeat 50 times, with a scripted field at the bottom:
/// 50⁵ instances if nothing stopped the walk.
fn nested_repeats_template() -> String {
    let mut body = r#"<field name="leaf" w="1in" h="0.3in"><ui><textEdit/></ui><calculate><script contentType="application/x-javascript">1</script></calculate></field>"#.to_string();
    for level in (0..5).rev() {
        body = format!(r#"<subform name="s{level}" layout="tb"><occur min="1" max="-1" initial="50"/>{body}</subform>"#);
    }
    format!(
        r#"<?xml version="1.0"?><xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb"><pageSet><pageArea name="p"><contentArea x="0" y="0" w="8in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>{body}</subform></template></xdp:xdp>"#
    )
}

#[test]
fn the_live_form_of_nested_repeating_subforms_stops_at_its_node_budget() {
    fn count(n: &crate::script::FormNode) -> usize {
        1 + n.children.iter().map(count).sum::<usize>()
    }
    let (tpl, _) = parse(&nested_repeats_template()).unwrap();
    assert!(crate::script::has_scripts(&tpl));
    let started = std::time::Instant::now();
    let live = form_tree(&tpl, None, &Overrides::default());
    assert!(live.truncated, "the walk was cut short");
    assert!(count(&live.root) <= crate::script::MAX_FORM_NODES + 1, "{} nodes", count(&live.root));
    assert!(live.events.len() <= 20_000);
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    // A template without scripts has nothing to run.
    let plain_xml = nested_repeats_template().replace(r#"<calculate><script contentType="application/x-javascript">1</script></calculate>"#, "");
    let (plain, _) = parse(&plain_xml).unwrap();
    assert!(!crate::script::has_scripts(&plain));
}

#[test]
fn data_ops_of_one_event_are_one_datasets_rewrite() {
    use crate::data::{DataOp, write_data_ops};
    let mut doc = Document::open(Arc::new(shell(&scripted_template()))).unwrap();
    render_into(&mut doc).unwrap();
    let before = doc.object_numbers().len();
    let ops = vec![
        DataOp::Value { path: som_to_path("form[0].page1[0].qty[0]"), text: "1".into() },
        DataOp::Value { path: som_to_path("form[0].page1[0].qty[0]"), text: "7".into() },
        DataOp::Instances { parent: som_to_path("form[0].page1[0].table[0]"), name: "row".into(), count: 3 },
        DataOp::Value { path: som_to_path("form[0].page1[0].table[0].row[2].amount[0]"), text: "5".into() },
        DataOp::Remove(som_to_path("form[0].page1[0].table[0].row[0]")),
    ];
    let r = write_data_ops(&mut doc, &ops, None).unwrap();
    assert!(r.written && r.stream.is_some(), "{r:?}");
    assert_eq!(doc.object_numbers().len(), before + 1, "one new datasets stream");
    let data = crate::data_of(&doc).unwrap();
    assert_eq!(data.text_at(&som_to_path("form[0].page1[0].qty[0]")), Some("7"), "the last value wins");
    assert_eq!(data.count(&som_to_path("form[0].page1[0].table[0]"), "row"), 2);
    assert_eq!(data.text_at(&som_to_path("form[0].page1[0].table[0].row[1].amount[0]")), Some("5"));
    // The in-memory model agrees with what was written.
    let mut model = DataNode::default();
    model.children.push(DataNode { name: "form".into(), ..Default::default() });
    for op in &ops {
        model.apply(op);
    }
    assert_eq!(model.text_at(&som_to_path("form[0].page1[0].qty[0]")), Some("7"));
    assert_eq!(model.count(&som_to_path("form[0].page1[0].table[0]"), "row"), 2);
    assert_eq!(model.text_at(&som_to_path("form[0].page1[0].table[0].row[1].amount[0]")), Some("5"));
    // A second write for the same event replaces that stream instead of adding another; an
    // empty value clears the node.
    let again = write_data_ops(&mut doc, &[DataOp::Value { path: som_to_path("form[0].page1[0].qty[0]"), text: String::new() }], r.stream).unwrap();
    assert_eq!(again.stream, r.stream);
    assert_eq!(doc.object_numbers().len(), before + 1);
    assert_eq!(crate::data_of(&doc).unwrap().text_at(&som_to_path("form[0].page1[0].qty[0]")), Some(""));
}

#[test]
fn script_buttons_carry_no_javascript_action_for_other_viewers() {
    let mut doc = Document::open(Arc::new(shell(&scripted_template()))).unwrap();
    render_into(&mut doc).unwrap();
    let fields = top_fields(&doc);
    let add_row = fields.iter().find(|(name, _)| name == "addRow").map(|(_, d)| d.clone()).expect("addRow");
    assert!(add_row.contains(crate::CLICK_KEY), "found by its SOM path");
    assert!(add_row.get(b"A").is_none() && add_row.get(b"AA").is_none(), "no XFA source as a PDF JavaScript action");
}

/// The generated fields' dictionaries by name (top-level fields only).
fn top_fields(doc: &Document) -> Vec<(String, pdfcraft_cos::Dict)> {
    let root = doc.root().unwrap();
    let catalog = doc.get(root);
    let acro = doc.resolve(catalog.as_dict().unwrap().get(b"AcroForm").unwrap());
    let fields = doc.resolve(acro.as_dict().unwrap().get(b"Fields").unwrap());
    fields
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| {
            let d = doc.resolve(f).as_dict().cloned()?;
            let t = d.get(b"T").and_then(|t| t.as_string()).map(|t| t.to_text())?;
            Some((t, d))
        })
        .collect()
}

#[test]
fn choice_lists_signatures_passwords_and_pictures_become_fields_and_images() {
    let form = layout_xml(&fields_template("")).unwrap();
    let p1 = &form.pages[0];
    let w = widgets(p1);
    let country = w.iter().find(|w| w.name == "country").unwrap();
    assert_eq!(
        country.kind,
        WidgetKind::Choice {
            options: vec![("CA".into(), "Canada".into()), ("FR".into(), "France".into()), ("JP".into(), "Japan".into())],
            list_box: false,
            multi: false,
            editable: false,
        }
    );
    assert_eq!(country.value.as_deref(), Some("FR"), "the template default, as a saved value");
    let langs = w.iter().find(|w| w.name == "langs").unwrap();
    assert!(
        matches!(&langs.kind, WidgetKind::Choice { options, list_box: true, multi: true, editable: false } if options.len() == 3 && options[0] == ("English".into(), "English".into()))
    );
    let other = w.iter().find(|w| w.name == "other").unwrap();
    assert!(matches!(&other.kind, WidgetKind::Choice { list_box: false, multi: false, editable: true, .. }));
    assert_eq!(w.iter().find(|w| w.name == "pin").unwrap().kind, WidgetKind::Password);
    assert_eq!(w.iter().find(|w| w.name == "sign").unwrap().kind, WidgetKind::Signature);
    assert!(w.iter().all(|w| w.name != "photo" && w.name != "code"), "image and barcode fields make no widget");
    // The PNG in the image field and the GIF in the draw are pictures, fitted to their boxes.
    let images: Vec<(&Rect, usize)> =
        p1.items.iter().filter_map(|i| if let Item::Image { rect, data, .. } = i { Some((rect, data.len())) } else { None }).collect();
    assert_eq!(images.len(), 2, "{images:?}");
    assert!((images[0].0.w - 72.0).abs() < 0.01 && (images[0].0.h - 72.0).abs() < 0.01, "a square picture fills a 1in square: {:?}", images[0].0);
    assert_eq!(images[1].1, TINY_GIF.len());
    let t = texts(p1);
    assert!(t.iter().any(|s| s == "12345"), "the barcode's value as text: {t:?}");
    assert!(form.warnings.iter().any(|w| w.contains("barcode")) && form.warnings.iter().any(|w| w.contains("image fields")), "{:?}", form.warnings);

    // Written into the PDF: /Ch fields with their options and flags, a /Sig field, a password
    // field, and Flate images (the PNG with a soft mask).
    let mut doc = Document::open(Arc::new(shell(&fields_template("")))).unwrap();
    let report = render_into(&mut doc).unwrap();
    assert_eq!(report.fields, 6, "country, langs, other, pin, sign, summary");
    let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let doc = Document::open(Arc::new(bytes)).unwrap();
    let fields = pdfcraft_forms_free_fields(&doc);
    let field = |n: &str| fields.iter().find(|(name, _)| name == n).map(|(_, d)| d.clone()).unwrap_or_else(|| panic!("no field {n}"));
    let country = field("country");
    assert_eq!(country.name(b"FT"), Some(b"Ch".as_slice()));
    assert_eq!(country.int(b"Ff"), Some(1 << 17), "a combo box");
    let opt = doc.resolve(country.get(b"Opt").unwrap()).as_array().unwrap().clone();
    assert_eq!(opt.len(), 3);
    let pair = doc.resolve(&opt[1]).as_array().unwrap().clone();
    assert_eq!(pair[0].as_string().unwrap().to_text(), "FR");
    assert_eq!(pair[1].as_string().unwrap().to_text(), "France");
    assert_eq!(country.get(b"V").and_then(|v| v.as_string()).map(|s| s.to_text()).as_deref(), Some("FR"));
    assert_eq!(country.get(b"DV").and_then(|v| v.as_string()).map(|s| s.to_text()).as_deref(), Some("FR"));
    let langs = field("langs");
    assert_eq!(langs.int(b"Ff"), Some(1 << 21), "a multi-select list box");
    let opt = doc.resolve(langs.get(b"Opt").unwrap()).as_array().unwrap().clone();
    assert_eq!(opt[0].as_string().unwrap().to_text(), "English", "same shown text and value: a plain string");
    assert_eq!(field("other").int(b"Ff"), Some((1 << 17) | (1 << 18)), "an editable combo box");
    assert_eq!(field("pin").int(b"Ff"), Some(1 << 13), "a password field");
    assert_eq!(field("sign").name(b"FT"), Some(b"Sig".as_slice()));
    let streams: Vec<pdfcraft_cos::Stream> = doc
        .object_numbers()
        .into_iter()
        .filter_map(|n| if let Object::Stream(st) = &*doc.get(pdfcraft_cos::ObjRef { num: n, generation: 0 }) { Some(st.clone()) } else { None })
        .filter(|s| s.dict.name(b"Subtype") == Some(b"Image"))
        .collect();
    assert_eq!(streams.len(), 3, "the PNG, its soft mask, the GIF");
    let png = streams.iter().find(|s| s.dict.contains(b"SMask")).expect("the PNG has a soft mask");
    assert_eq!(png.dict.name(b"ColorSpace"), Some(b"DeviceRGB".as_slice()));
    assert_eq!(png.dict.name(b"Filter"), Some(b"FlateDecode".as_slice()));
    assert_eq!(png.decoded().unwrap().len(), 12, "2 × 2 RGB samples");
    let gif = streams.iter().find(|s| s.dict.int(b"Width") == Some(1)).expect("the 1 × 1 GIF");
    assert_eq!(gif.dict.name(b"ColorSpace"), Some(b"DeviceGray".as_slice()), "a white pixel is gray");
    assert_eq!(gif.decoded().unwrap(), vec![255]);
}

/// The terminal field dictionaries by name (one level: this fixture has no hierarchy).
fn pdfcraft_forms_free_fields(doc: &Document) -> Vec<(String, pdfcraft_cos::Dict)> {
    let root = doc.get(doc.root().unwrap());
    let acro = doc.resolve(root.as_dict().unwrap().get(b"AcroForm").unwrap());
    let fields = doc.resolve(acro.as_dict().unwrap().get(b"Fields").unwrap()).as_array().unwrap().clone();
    fields
        .iter()
        .filter_map(|f| {
            let d = doc.resolve(f).as_dict()?.clone();
            let name = d.get(b"T").map(|t| doc.resolve(t)).and_then(|t| t.as_string().map(|s| s.to_text()))?;
            Some((name, d))
        })
        .collect()
}

#[test]
fn choice_values_come_from_the_data_as_saved_values_and_several_for_lists() {
    // Shown text in the data is taken too (an editable list the user typed into saves text).
    let xml = fields_template("<form><page1><country>Japan</country><langs>English\nSpanish</langs><other>typed</other></page1></form>");
    let form = layout_xml(&xml).unwrap();
    let w = widgets(&form.pages[0]);
    assert_eq!(w.iter().find(|w| w.name == "country").unwrap().value.as_deref(), Some("JP"));
    assert_eq!(w.iter().find(|w| w.name == "langs").unwrap().value.as_deref(), Some("English\nSpanish"));
    assert_eq!(w.iter().find(|w| w.name == "other").unwrap().value.as_deref(), Some("typed"));
    let mut doc = Document::open(Arc::new(shell(&xml))).unwrap();
    render_into(&mut doc).unwrap();
    let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let doc = Document::open(Arc::new(bytes)).unwrap();
    let fields = pdfcraft_forms_free_fields(&doc);
    let langs = &fields.iter().find(|(n, _)| n == "langs").unwrap().1;
    let v = doc.resolve(langs.get(b"V").unwrap()).as_array().unwrap().clone();
    assert_eq!(v.iter().map(|s| s.as_string().unwrap().to_text()).collect::<Vec<_>>(), vec!["English", "Spanish"]);
    let i = doc.resolve(langs.get(b"I").unwrap()).as_array().unwrap().clone();
    assert_eq!(i.iter().map(|o| o.as_int().unwrap()).collect::<Vec<_>>(), vec![0, 2]);
}

#[test]
fn pictures_are_sized_from_their_headers_and_hostile_ones_are_refused_unallocated() {
    use crate::image::{Kind, decode, kind, size};
    assert_eq!(kind(&tiny_png()), Some(Kind::Png));
    assert_eq!(size(&tiny_png()), Some((2, 2)));
    assert_eq!(kind(TINY_GIF), Some(Kind::Gif));
    assert_eq!(size(TINY_GIF), Some((1, 1)));
    assert_eq!(kind(b"BM..."), None);
    assert_eq!(size(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), None, "a header cut short");
    let decoded = decode(&tiny_png()).unwrap();
    assert_eq!((decoded.width, decoded.height, decoded.color_space), (2, 2, "DeviceRGB"));
    assert_eq!(decoded.samples, vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0]);
    assert_eq!(decoded.alpha, Some(vec![255, 255, 255, 0]));
    assert_eq!(decode(TINY_GIF).unwrap().samples, vec![255]);
    // A PNG claiming 100,000 × 100,000 pixels is refused from its header, before any buffer.
    let mut huge = tiny_png();
    huge[16..20].copy_from_slice(&100_000u32.to_be_bytes());
    huge[20..24].copy_from_slice(&100_000u32.to_be_bytes());
    assert_eq!(size(&huge), None, "no size for layout: the box is used");
    assert!(decode(&huge).unwrap_err().contains("million pixels"));
    // Truncated and corrupt data are errors, not panics.
    let cut = &tiny_png()[..30];
    assert!(decode(cut).is_err());
    let mut bad = tiny_png();
    for b in bad.iter_mut().skip(40) {
        *b ^= 0x55;
    }
    assert!(decode(&bad).is_err() || decode(&bad).is_ok());
    let mut gif = TINY_GIF.to_vec();
    gif.truncate(12);
    assert!(decode(&gif).is_err());
    // A form whose picture is broken still lays out, with a warning, and keeps the rest.
    let xml = fields_template("")
        .replace("image/gif", "image/gif\" transferEncoding=\"none")
        .replace("<image contentType=\"image/png\">", "<image contentType=\"image/png\">AAAA");
    let mut doc = Document::open(Arc::new(shell(&xml))).unwrap();
    let report = render_into(&mut doc).unwrap();
    assert_eq!(report.fields, 6);
    assert!(report.warnings.iter().any(|w| w.contains("image")), "{:?}", report.warnings);
}

#[test]
fn choice_values_are_saved_values_first_deduplicated_and_defaults_are_saved_values_too() {
    // Designer's "specify item values": shown 0..3, saved 1..4. The data's 1 means the item
    // shown as 0, not the item shown as 1.
    let tpl = fields_template("<form><page1><country>1</country></page1></form>")
        .replace("<text>Canada</text><text>France</text><text>Japan</text>", "<text>0</text><text>1</text><text>2</text><text>3</text>")
        .replace("<text>CA</text><text>FR</text><text>JP</text>", "<text>1</text><text>2</text><text>3</text><text>4</text>")
        .replace("<value><text>FR</text></value></field>", "<value><text>1</text></value></field>");
    let form = layout_xml(&tpl).unwrap();
    let w = widgets(&form.pages[0]);
    assert_eq!(w.iter().find(|w| w.name == "country").unwrap().value.as_deref(), Some("1"));
    let (t, _) = parse(&tpl).unwrap();
    let tree = form_tree(&t, parse_datasets(&tpl).as_ref(), &Overrides::default());
    let node = tree.root.children.iter().find(|n| n.name == "page1").unwrap().children.iter().find(|n| n.name == "country").unwrap();
    assert_eq!(node.value, "1", "scripts read the saved value");
    // Several values: each once, never more than there are items, written as ascending indices;
    // a single-select list takes the first line; defaults are saved values.
    let tpl = fields_template("<form><page1><country>Japan\nCanada</country><langs>Spanish\nEnglish\nEnglish\nSpanish</langs></page1></form>")
        .replace("<value><text>FR</text></value></field>", "<value><text>France</text></value></field>")
        .replace(
            "<items><text>English</text><text>French</text><text>Spanish</text></items>",
            "<items><text>English</text><text>French</text><text>Spanish</text></items><value><text>English\nFrench</text></value>",
        );
    let form = layout_xml(&tpl).unwrap();
    let w = widgets(&form.pages[0]);
    assert_eq!(w.iter().find(|w| w.name == "langs").unwrap().value.as_deref(), Some("Spanish\nEnglish"));
    assert_eq!(w.iter().find(|w| w.name == "country").unwrap().value.as_deref(), Some("JP"));
    assert_eq!(w.iter().find(|w| w.name == "country").unwrap().default.as_deref(), Some("FR"));
    let mut doc = Document::open(Arc::new(shell(&tpl))).unwrap();
    render_into(&mut doc).unwrap();
    let doc = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let fields = pdfcraft_forms_free_fields(&doc);
    let langs = &fields.iter().find(|(n, _)| n == "langs").unwrap().1;
    let i = doc.resolve(langs.get(b"I").unwrap()).as_array().unwrap().clone();
    assert_eq!(i.iter().map(|o| o.as_int().unwrap()).collect::<Vec<_>>(), vec![0, 2]);
    let dv = doc.resolve(langs.get(b"DV").unwrap()).as_array().unwrap().clone();
    assert_eq!(dv.iter().map(|s| s.as_string().unwrap().to_text()).collect::<Vec<_>>(), vec!["English", "French"]);
    let country = &fields.iter().find(|(n, _)| n == "country").unwrap().1;
    assert_eq!(country.get(b"DV").and_then(|v| v.as_string()).map(|s| s.to_text()).as_deref(), Some("FR"));
    assert_eq!(doc.resolve(country.get(b"I").unwrap()).as_array().unwrap().len(), 1);
    // Picks stored as <value> children are read too.
    let tpl = fields_template("<form><page1><langs><value>English</value><value>Spanish</value></langs></page1></form>");
    let form = layout_xml(&tpl).unwrap();
    assert_eq!(widgets(&form.pages[0]).iter().find(|w| w.name == "langs").unwrap().value.as_deref(), Some("English\nSpanish"));
    // More items than the cap: kept to the cap, and said so.
    let many: String = (0..1500).map(|i| format!("<text>i{i}</text>")).collect();
    let tpl = fields_template("").replace("<items><text>A</text><text>B</text></items>", &format!("<items>{many}</items>"));
    let form = layout_xml(&tpl).unwrap();
    let other = widgets(&form.pages[0]).into_iter().find(|w| w.name == "other").unwrap();
    assert!(matches!(&other.kind, WidgetKind::Choice { options, .. } if options.len() == 1000));
    assert!(form.warnings.iter().any(|w| w.contains("other") && w.contains("1000 items")), "{:?}", form.warnings);
}

#[test]
fn pictures_are_embedded_once_across_relayouts_and_bad_data_pictures_are_reported() {
    use crate::image::{decode, size};
    let tpl = fields_template("<form><page1><country>JP</country></page1></form>");
    let mut doc = Document::open(Arc::new(shell(&tpl))).unwrap();
    render_into(&mut doc).unwrap();
    let (t, _) = parse(&tpl).unwrap();
    rerender(&mut doc, &t).unwrap();
    rerender(&mut doc, &t).unwrap();
    let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let doc = Document::open(Arc::new(bytes)).unwrap();
    let images = doc
        .object_numbers()
        .into_iter()
        .filter(
            |n| matches!(&*doc.get(pdfcraft_cos::ObjRef { num: *n, generation: 0 }), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image")),
        )
        .count();
    assert_eq!(images, 3, "the PNG, its soft mask and the GIF, however many times the form was laid out");
    // No private map in the catalog: earlier pictures are found on the pages, by their hash.
    let root = doc.get(doc.root().unwrap());
    assert!(!root.as_dict().unwrap().contains(b"PCXfaImages"));
    // Pictures in the data that aren't pictures: a warning naming the field, the template's
    // picture shown instead.
    for (data, what) in [("Qk0AAAAA", "not a JPEG, PNG or GIF"), ("not base64!", "not base64")] {
        let tpl = fields_template(&format!("<form><page1><photo>{data}</photo></page1></form>"));
        let form = layout_xml(&tpl).unwrap();
        assert!(form.warnings.iter().any(|w| w.contains("photo") && w.contains(what)), "{what}: {:?}", form.warnings);
        assert_eq!(form.pages[0].items.iter().filter(|i| matches!(i, Item::Image { .. })).count(), 2);
    }
    // A hostile PNG header: no size for layout (the draw keeps its box), refused at decode.
    let mut tall = tiny_png();
    tall[16..20].copy_from_slice(&1u32.to_be_bytes());
    tall[20..24].copy_from_slice(&4_000_000_000u32.to_be_bytes());
    assert_eq!(size(&tall), None);
    assert!(decode(&tall).unwrap_err().contains("million pixels"));
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&tall);
    let tpl = fields_template("").replace(
        r#"<draw name="logo" w="0.5in" h="0.5in">"#,
        &format!(
            r#"<draw name="tall" w="2in"><value><image contentType="image/png">{b64}</image></value></draw><draw name="logo" w="0.5in" h="0.5in">"#
        ),
    );
    let form = layout_xml(&tpl).unwrap();
    assert_eq!(form.pages.len(), 1, "a picture of unknown size takes its box, not a page");
    // A GIF whose frame is far larger than its 1 × 1 screen is refused before it is allocated.
    let mut gif = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\xff\xff\xff\x00\x00\x00,\x00\x00\x00\x00".to_vec();
    gif.extend_from_slice(&11_000u16.to_le_bytes());
    gif.extend_from_slice(&11_000u16.to_le_bytes());
    gif.extend_from_slice(b"\x00\x02\x02D\x01\x00;");
    let started = std::time::Instant::now();
    assert!(decode(&gif).unwrap_err().contains("million pixels"));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    // A template picture over 1 MiB is read whole (text is cut at 1 MiB, pictures are not).
    let big = "A".repeat(1_600_000);
    let tpl = fields_template("").replace(r#"<image contentType="image/gif">"#, &format!(r#"<image contentType="image/gif">{big}"#));
    let (t, _) = parse(&tpl).unwrap();
    fn find_image(nodes: &[Node]) -> Option<usize> {
        nodes.iter().find_map(|n| match n {
            Node::Draw(d) => match &d.value {
                Value::Image { data, .. } => Some(data.len()),
                _ => None,
            },
            Node::Subform(s) => find_image(&s.children),
            _ => None,
        })
    }
    assert!(find_image(&t.root.children).is_some_and(|n| n > 1_200_000));
}

/// A repeating subform drawn inside positioned content (a top-to-bottom table in an area of a
/// positioned subform) gets its `occur` instances, as in Adobe's viewers: real Designer forms
/// have tables like this whose calculations read every row. Before, only one row was laid
/// out, and the field after the table was placed as if there were one.
#[test]
fn repeating_subforms_inside_positioned_content_get_their_instances() {
    let xdp = r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="page1" w="7.5in" h="8in">
  <area name="box" x="0.5in" y="1in">
   <subform name="wrap" layout="tb" w="4in">
    <subform name="table" layout="tb" w="4in">
     <subform name="row" w="4in" h="0.3in"><occur min="3" max="-1"/>
      <field name="reason" w="4in" h="0.3in"><ui><textEdit/></ui></field>
     </subform>
    </subform>
    <field name="after" w="4in" h="0.3in"><ui><textEdit/></ui></field>
   </subform>
  </area>
 </subform>
</subform>
</template>
</xdp:xdp>
"##;
    let form = layout_xml(xdp).unwrap();
    let all: Vec<&Widget> = form.pages.iter().flat_map(widgets).collect();
    let rows: Vec<&Widget> = all.iter().copied().filter(|w| w.som.ends_with(".reason[0]")).collect();
    let soms: Vec<&str> = rows.iter().map(|w| w.som.as_str()).collect();
    assert_eq!(
        soms,
        [
            "form[0].page1[0].box[0].wrap[0].table[0].row[0].reason[0]",
            "form[0].page1[0].box[0].wrap[0].table[0].row[1].reason[0]",
            "form[0].page1[0].box[0].wrap[0].table[0].row[2].reason[0]",
        ],
        "one widget per instance, each with its own SOM index"
    );
    // The rows are stacked, not drawn on top of each other…
    let step = rows[1].rect.y - rows[0].rect.y;
    assert!(step.abs() > 1.0, "rows overlap: {:?}", rows.iter().map(|w| w.rect).collect::<Vec<_>>());
    assert!(((rows[2].rect.y - rows[1].rect.y) - step).abs() < 0.01, "rows are evenly spaced");
    // …and what follows the table is placed after all of them (the table was measured with
    // its instances, not with one row).
    let after = all.iter().find(|w| w.som.ends_with(".after[0]")).expect("the field after the table");
    assert!(((after.rect.y - rows[2].rect.y) - step).abs() < 0.01, "after: {:?}, last row: {:?}", after.rect, rows[2].rect);
}

/// Areas are named in SOM expressions but are not data scopes; unnamed subforms and areas have
/// no segment at all. What the template doesn't have is kept as it is.
#[test]
fn data_paths_leave_areas_out() {
    let (tpl, _) = parse(&crate::fixtures::leave_template("")).unwrap();
    let path = |som: &str| data_path(&tpl, som).into_iter().map(|(n, i)| format!("{n}[{i}]")).collect::<Vec<_>>().join(".");
    // The area `box` (inside an unnamed subform) is dropped; indexes are kept.
    assert_eq!(path("form[0].page[0].box[0].table[0].row[2].days[0]"), "form[0].page[0].table[0].row[2].days[0]");
    assert_eq!(path("form[0].page[0].box[0].rest[0]"), "form[0].page[0].rest[0]");
    // No area on the way: as som_to_path.
    assert_eq!(path("form[0].page[0]"), "form[0].page[0]");
    // Unknown segments, past a leaf, or a SOM from another template: kept.
    assert_eq!(path("form[0].page[0].box[0].nothing[0].x[1]"), "form[0].page[0].nothing[0].x[1]");
    assert_eq!(path("form[0].page[0].box[0].rest[0].box[0]"), "form[0].page[0].rest[0].box[0]");
    assert_eq!(path("other[0].box[0].rest[0]"), "other[0].box[0].rest[0]");
    assert_eq!(data_path(&tpl, ""), Vec::new());
}
