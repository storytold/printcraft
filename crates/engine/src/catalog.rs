//! The tool catalogue: Acrobat's "All tools" information architecture, as data.
//!
//! Every UI (the egui shell today, anything tomorrow) builds its tool panel, palette and menus
//! from this table; nothing is hard-coded in the toolkit. Each item names the command it will
//! dispatch and the milestone (plan/execution-plan.md) in which it ships, so the UI can be honest
//! about what works today.
//!
//! Source of the inventory: plan/acrobat/02-ui-ux.md §2 (observed in Acrobat Pro 26.002).

/// Where a tool or item stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// Works in this build.
    Ready,
    /// Planned; ships in the named milestone.
    Planned(&'static str),
    /// Cloud-only in Acrobat; PdfCraft offers an optional pluggable provider instead.
    Provider,
}

#[derive(Clone, Copy, Debug)]
pub struct ToolItem {
    pub label: &'static str,
    pub icon: &'static str,
    pub command: &'static str,
    pub availability: Availability,
}

#[derive(Clone, Copy, Debug)]
pub struct ToolSection {
    pub title: &'static str,
    pub items: &'static [ToolItem],
}

#[derive(Clone, Copy, Debug)]
pub struct ToolGroup {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    /// Icon tint, as Acrobat colour-codes its tool families.
    pub hue: [u8; 3],
    pub badge: Option<&'static str>,
    pub availability: Availability,
    pub sections: &'static [ToolSection],
}

const fn item(label: &'static str, icon: &'static str, command: &'static str, availability: Availability) -> ToolItem {
    ToolItem { label, icon, command, availability }
}

use Availability::{Planned, Provider, Ready};

const RED: [u8; 3] = [0xE0, 0x3E, 0x3E];
const PURPLE: [u8; 3] = [0x8E, 0x4E, 0xE6];
const BLUE: [u8; 3] = [0x3A, 0x6F, 0xE8];
const GREEN: [u8; 3] = [0x2D, 0x9D, 0x5B];
const ORANGE: [u8; 3] = [0xE8, 0x8A, 0x1A];
const TEAL: [u8; 3] = [0x14, 0x9C, 0xA8];
const PINK: [u8; 3] = [0xD6, 0x3B, 0x8F];

pub static TOOL_GROUPS: &[ToolGroup] = &[
    ToolGroup {
        id: "export",
        label: "Export a PDF",
        icon: "file-output",
        hue: BLUE,
        badge: None,
        availability: Ready,
        sections: &[ToolSection {
            title: "Export to",
            items: &[
                item("Microsoft Word (.docx)", "file-text", "export.docx", Ready),
                item("Spreadsheet (.xlsx)", "grid-3x3", "export.xlsx", Planned("M10")),
                item("Presentation (.pptx)", "presentation", "export.pptx", Planned("M10")),
                item("Image (PNG)", "image", "export.image", Ready),
                item("Export all images", "image", "export.all_images", Ready),
                item("HTML web page", "file-symlink", "export.html", Ready),
                item("Rich Text Format (.rtf)", "file-text", "export.rtf", Ready),
                item("Text (plain)", "type", "export.text", Ready),
                item("PostScript / EPS", "file-down", "export.ps", Planned("M10")),
            ],
        }],
    },
    ToolGroup {
        id: "fill_sign",
        label: "Fill & Sign",
        icon: "pen-line",
        hue: PURPLE,
        badge: None,
        availability: Ready,
        sections: &[ToolSection {
            title: "Fill",
            items: &[
                item("Add text", "type", "sign.fill.text", Ready),
                item("Checkmark", "check", "sign.fill.check", Ready),
                item("Cross", "x", "sign.fill.cross", Ready),
                item("Dot", "circle-dot", "sign.fill.dot", Ready),
                item("Line", "minus", "sign.fill.line", Ready),
                item("Date", "clock-3", "sign.fill.date", Ready),
                item("Signature", "signature", "sign.fill.signature", Ready),
                item("Initials", "signature", "sign.fill.initials", Ready),
                item("Change signature", "signature", "sign.fill.signature.change", Ready),
                item("Change initials", "signature", "sign.fill.initials.change", Ready),
            ],
        }],
    },
    ToolGroup {
        id: "edit",
        label: "Edit a PDF",
        icon: "file-pen-line",
        hue: PINK,
        badge: None,
        availability: Planned("M7"),
        sections: &[
            ToolSection {
                title: "Modify page",
                items: &[
                    item("Rotate pages", "rotate-cw", "page.rotate", Ready),
                    item("Insert pages", "file-plus-2", "page.insert", Ready),
                    item("Delete pages", "trash-2", "page.delete", Ready),
                    item("Extract pages", "file-output", "page.extract", Ready),
                    item("Organize pages", "layout-grid", "page.organize", Ready),
                ],
            },
            ToolSection {
                title: "Add content",
                items: &[
                    item("Edit text & images", "text-cursor-input", "edit.edit_text", Ready),
                    item("Text", "type", "edit.text", Ready),
                    item("Image", "image-plus", "edit.image", Ready),
                    item("Header and footer", "heading", "edit.header_footer", Ready),
                    item("Watermark", "stamp", "edit.watermark", Ready),
                    item("Link", "link-2", "edit.link", Ready),
                    item("Bates numbering", "hash", "edit.bates", Ready),
                    item("Background", "palette", "edit.background", Ready),
                    item("Attach file", "paperclip", "edit.attach", Planned("M12")),
                ],
            },
        ],
    },
    ToolGroup {
        id: "create",
        label: "Create a PDF",
        icon: "file-plus-2",
        hue: RED,
        badge: None,
        availability: Ready,
        sections: &[ToolSection {
            title: "Create from",
            items: &[
                item("Single file", "file-input", "create.file", Ready),
                item("Multiple files", "files", "create.multiple", Ready),
                item("Images", "image", "create.images", Ready),
                item("Clipboard", "copy-plus", "create.clipboard", Ready),
                item("Blank page", "file-plus-2", "create.blank", Ready),
            ],
        }],
    },
    ToolGroup {
        id: "combine",
        label: "Combine files",
        icon: "files",
        hue: BLUE,
        badge: None,
        availability: Ready,
        sections: &[ToolSection { title: "Combine", items: &[item("Add files to combine", "files", "page.combine", Ready)] }],
    },
    ToolGroup {
        id: "organize",
        label: "Organize pages",
        icon: "layout-grid",
        hue: GREEN,
        badge: None,
        availability: Ready,
        sections: &[
            ToolSection {
                title: "Page options",
                items: &[
                    item("Page grid", "layout-grid", "page.organize", Ready),
                    item("Rotate", "rotate-cw", "page.rotate", Ready),
                    item("Delete", "trash-2", "page.delete", Ready),
                    item("Extract", "file-output", "page.extract", Ready),
                    item("Insert", "file-plus-2", "page.insert", Ready),
                    item("Replace", "replace", "page.replace", Ready),
                    item("Split", "scissors", "page.split", Ready),
                ],
            },
            ToolSection {
                title: "More",
                items: &[
                    item("Set page boxes", "square-dashed-mouse-pointer", "page.boxes", Ready),
                    item("Crop pages", "crop", "page.crop", Ready),
                    item("Duplicate pages", "copy-plus", "page.duplicate", Ready),
                    item("Page labels", "tag", "page.number", Ready),
                    item("Page transitions", "presentation", "page.transitions", Planned("M4")),
                ],
            },
        ],
    },
    ToolGroup {
        id: "comment",
        label: "Add comments",
        icon: "message-square-text",
        hue: ORANGE,
        badge: None,
        availability: Ready,
        sections: &[
            ToolSection {
                title: "Markup",
                items: &[
                    item("Sticky note", "sticky-note", "comment.note", Ready),
                    item("Highlight text", "highlighter", "comment.highlight", Ready),
                    item("Underline", "underline", "comment.underline", Ready),
                    item("Strikethrough", "strikethrough", "comment.strikeout", Ready),
                    item("Squiggly underline", "spline", "comment.squiggly", Ready),
                    item("Text box", "type", "comment.freetext", Ready),
                ],
            },
            ToolSection {
                title: "Drawing",
                items: &[
                    item("Draw freehand", "pencil", "comment.ink", Ready),
                    item("Rectangle", "square", "comment.square", Ready),
                    item("Oval", "circle", "comment.circle", Ready),
                    item("Line", "minus", "comment.line", Ready),
                    item("Arrow", "move-right", "comment.arrow", Ready),
                    item("Cloud", "cloud", "comment.cloud", Planned("M5")),
                    item("Stamp", "stamp", "comment.stamp", Planned("M5")),
                ],
            },
            ToolSection {
                title: "Review",
                items: &[
                    item("Comment list", "message-square-text", "comment.list", Ready),
                    item("Flatten comments", "layers", "comment.flatten", Ready),
                ],
            },
        ],
    },
    ToolGroup {
        id: "scan",
        label: "Scan & OCR",
        icon: "scan-text",
        hue: GREEN,
        badge: None,
        availability: Ready,
        sections: &[ToolSection {
            title: "Recognize text",
            items: &[
                item("In this file", "scan-text", "ocr.recognize", Ready),
                item("In multiple files", "files", "ocr.recognize_batch", Ready),
                item("Enhance scanned file", "sparkles", "ocr.enhance", Planned("M10")),
                item("Correct recognized text", "text-select", "ocr.correct", Planned("M10")),
            ],
        }],
    },
    ToolGroup {
        id: "protect",
        label: "Protect a PDF",
        icon: "shield-check",
        hue: BLUE,
        badge: None,
        availability: Planned("M8"),
        sections: &[
            ToolSection {
                title: "Protect",
                items: &[
                    item("Protect with password", "lock", "protect.password", Ready),
                    item("Remove hidden information", "eye-off", "protect.remove_hidden", Ready),
                ],
            },
            ToolSection {
                title: "Advanced options",
                items: &[
                    item("Encrypt with certificate", "file-lock-2", "protect.certificate", Planned("M8")),
                    item("Security properties", "shield-check", "protect.properties", Ready),
                    item("Remove security", "lock-open", "protect.remove", Ready),
                ],
            },
        ],
    },
    ToolGroup {
        id: "redact",
        label: "Redact a PDF",
        icon: "rectangle-horizontal",
        hue: RED,
        badge: None,
        availability: Ready,
        sections: &[ToolSection {
            title: "Redact",
            items: &[
                item("Redact text and images", "rectangle-horizontal", "redact.mark", Ready),
                item("Redact pages", "file-x", "redact.pages", Ready),
                item("Find text and redact", "file-search", "redact.search", Ready),
                item("Set properties", "settings-2", "redact.properties", Ready),
                item("Sanitize document", "sparkles", "redact.sanitize", Ready),
            ],
        }],
    },
    ToolGroup {
        id: "compress",
        label: "Compress a PDF",
        icon: "file-down",
        hue: RED,
        badge: None,
        availability: Ready,
        sections: &[ToolSection {
            title: "Optimize",
            items: &[
                item("Reduce file size", "file-down", "optimize.reduce", Ready),
                item("Advanced optimization", "settings-2", "optimize.advanced", Ready),
                item("Audit space usage", "gauge", "optimize.audit", Planned("M11")),
            ],
        }],
    },
    ToolGroup {
        id: "form",
        label: "Prepare a form",
        icon: "text-cursor-input",
        hue: PURPLE,
        badge: None,
        availability: Ready,
        sections: &[
            ToolSection {
                title: "Fields",
                items: &[
                    item("Field list", "list", "form.fields", Ready),
                    item("Detect form fields", "scan", "form.detect", Ready),
                    item("Clear form", "eraser", "form.clear", Ready),
                    item("Field properties", "settings-2", "form.field.properties", Ready),
                    item("Tab order by rows", "rows-3", "form.tab_order.row", Ready),
                    item("Tab order by columns", "columns-3", "form.tab_order.column", Ready),
                    item("Flatten form fields", "layers", "form.flatten", Ready),
                    item("Import data", "file-input", "form.import_data", Ready),
                    item("Export data", "file-output", "form.export_data", Ready),
                ],
            },
            ToolSection {
                title: "Add form components",
                items: &[
                    item("Text field", "text-cursor-input", "form.add.text", Ready),
                    item("Checkbox", "check-circle-2", "form.add.checkbox", Ready),
                    item("Radio button", "circle", "form.add.radio", Ready),
                    item("Drop-down list", "chevron-down", "form.add.combo", Ready),
                    item("List box", "list", "form.add.list", Ready),
                    item("Button", "square", "form.add.button", Ready),
                    item("Image field", "image", "form.add.image", Ready),
                    item("Date field", "clock-3", "form.add.date", Ready),
                    item("Digital signature", "signature", "form.add.signature", Ready),
                ],
            },
        ],
    },
    ToolGroup {
        id: "stamp",
        label: "Add a stamp",
        icon: "stamp",
        hue: PURPLE,
        badge: None,
        availability: Ready,
        sections: &[ToolSection { title: "Stamps", items: &[item("Stamp palette", "stamp", "comment.stamp", Ready)] }],
    },
    ToolGroup {
        id: "certificate",
        label: "Use a certificate",
        icon: "badge-check",
        hue: TEAL,
        badge: None,
        availability: Ready,
        sections: &[ToolSection {
            title: "Certificates",
            items: &[
                item("Digitally sign", "signature", "sign.digital", Ready),
                item("Timestamp", "clock-3", "sign.timestamp", Planned("M9")),
                item("Validate all signatures", "badge-check", "sign.validate", Ready),
                item("Verify digital signature", "badge-check", "sign.validate", Ready),
                item("Certify (visible signature)", "badge-check", "sign.certify", Ready),
                item("Certify (invisible signature)", "badge-check", "sign.certify_invisible", Ready),
            ],
        }],
    },
    ToolGroup {
        id: "print_production",
        label: "Use print production",
        icon: "printer",
        hue: PURPLE,
        badge: None,
        availability: Planned("M11"),
        sections: &[ToolSection {
            title: "Print production",
            items: &[
                item("Output preview", "layers", "prepress.output_preview", Planned("M11")),
                item("Preflight", "file-check", "preflight.run", Planned("M11")),
                item("Convert colors", "palette", "prepress.convert_colors", Planned("M11")),
                item("Set page boxes", "square-dashed-mouse-pointer", "page.boxes", Ready),
                item("Add printer marks", "ruler", "prepress.marks", Planned("M11")),
                item("Fix hairlines", "minus", "prepress.hairlines", Planned("M11")),
            ],
        }],
    },
    ToolGroup {
        id: "measure",
        label: "Measure objects",
        icon: "ruler",
        hue: PINK,
        badge: None,
        availability: Ready,
        sections: &[ToolSection {
            title: "Measure",
            items: &[
                item("Distance", "ruler", "measure.distance", Ready),
                item("Perimeter", "ruler", "measure.perimeter", Ready),
                item("Area", "ruler", "measure.area", Ready),
                item("Drawing scale", "ruler", "measure.scale", Ready),
                item("Measurement information", "ruler", "measure.info", Ready),
                item("Snapping", "ruler", "measure.snap", Ready),
                item("Export measurements", "file-output", "measure.export", Ready),
                item("Geospatial location", "compass", "measure.geo", Planned("M12")),
            ],
        }],
    },
    ToolGroup {
        id: "compare",
        label: "Compare files",
        icon: "git-compare",
        hue: PINK,
        badge: None,
        availability: Ready,
        sections: &[ToolSection { title: "Compare", items: &[item("Select files to compare", "git-compare", "doc.compare", Ready)] }],
    },
    ToolGroup {
        id: "actions",
        label: "Use guided actions",
        icon: "list-checks",
        hue: PURPLE,
        badge: None,
        availability: Ready,
        sections: &[ToolSection {
            title: "Actions list",
            items: &[
                item("Action Wizard", "list-checks", "actions.wizard", Ready),
                item("Make accessible", "accessibility", "actions.make_accessible", Planned("M13")),
                item("Prepare for distribution", "send", "actions.distribution", Ready),
                item("Optimize scanned documents", "scan-text", "actions.optimize_scans", Ready),
                item("Archive documents", "file-check", "actions.archive", Planned("M13")),
            ],
        }],
    },
    ToolGroup {
        id: "accessibility",
        label: "Prepare for accessibility",
        icon: "accessibility",
        hue: PURPLE,
        badge: None,
        availability: Ready,
        sections: &[ToolSection {
            title: "Accessibility",
            items: &[
                item("Automatically tag PDF", "tag", "a11y.autotag", Planned("M12")),
                item("Change reading options", "book-open", "a11y.reading_options", Ready),
                item("Check for accessibility", "accessibility", "a11y.check", Ready),
                item("Open accessibility report", "file-text", "a11y.report", Ready),
                item("Add alternate text", "image", "a11y.alt_text", Ready),
                item("Fix reading order", "list-ordered", "a11y.reading_order", Planned("M12")),
            ],
        }],
    },
    ToolGroup {
        id: "standards",
        label: "Apply PDF standards",
        icon: "file-check",
        hue: RED,
        badge: None,
        availability: Ready,
        sections: &[ToolSection {
            title: "Standards",
            items: &[
                item("Save as PDF/A", "file-check", "standards.pdfa", Ready),
                item("Save as PDF/X", "file-check", "standards.pdfx", Planned("M11")),
                item("Save as PDF/UA", "accessibility", "standards.pdfua", Planned("M11")),
                item("Preflight", "file-check", "preflight.run", Planned("M11")),
            ],
        }],
    },
    ToolGroup {
        id: "search_index",
        label: "Add search index",
        icon: "file-search",
        hue: GREEN,
        badge: None,
        availability: Planned("M12"),
        sections: &[ToolSection { title: "Index", items: &[item("Manage embedded index", "file-search", "search.index", Planned("M12"))] }],
    },
    ToolGroup {
        id: "javascript",
        label: "Use JavaScript",
        icon: "braces",
        hue: BLUE,
        badge: None,
        availability: Planned("M6"),
        sections: &[ToolSection {
            title: "JavaScript",
            items: &[
                item("Console", "braces", "js.console", Planned("M6")),
                item("Document JavaScripts", "file-text", "js.document", Planned("M6")),
                item("Document actions", "list-checks", "js.actions", Planned("M6")),
            ],
        }],
    },
    ToolGroup {
        id: "ai",
        label: "AI assistant",
        icon: "sparkles",
        hue: PINK,
        badge: Some("Optional"),
        availability: Provider,
        sections: &[ToolSection {
            title: "Bring your own model (off by default)",
            items: &[
                item("Summarize", "sparkles", "ai.summary", Provider),
                item("Ask about this document", "message-circle-reply", "ai.ask", Provider),
                item("Translate", "languages", "ai.translate", Provider),
            ],
        }],
    },
];

pub fn group(id: &str) -> Option<&'static ToolGroup> {
    TOOL_GROUPS.iter().find(|g| g.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_and_every_group_has_items() {
        let mut seen = HashSet::new();
        for g in TOOL_GROUPS {
            assert!(seen.insert(g.id), "duplicate group id {}", g.id);
            assert!(g.sections.iter().any(|s| !s.items.is_empty()), "{} has no items", g.id);
        }
    }

    #[test]
    fn commands_are_namespaced() {
        for g in TOOL_GROUPS {
            for s in g.sections {
                for i in s.items {
                    assert!(i.command.contains('.'), "{} lacks a namespace", i.command);
                }
            }
        }
    }
}
