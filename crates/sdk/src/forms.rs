//! forms.rs — Forms & Data Exchange domain models matching Datalogics APDFL architecture.
//! =====================================================================================
//!
//! Purpose:
//!     Defines Field, TextField, ButtonField, ChoiceField, SignatureField,
//!     AcroFormExportType, and AcroFormImportType matching Datalogics APDFL Form specifications.
//!
//! Layer:
//!     Domain Layer / Forms & Data Exchange
//!
//! Key Types:
//!     - AcroFormExportType / AcroFormImportType: Data formats (FDF, XFDF, XML, JSON)
//!     - Field: Base interactive AcroForm field descriptor
//!     - TextField: Text input widget (single/multiline/password)
//!     - ButtonField: Pushbutton, checkbox, or radio button widget
//!     - ChoiceField: Dropdown combo box or list selection widget
//!     - SignatureField: Digital signature placeholder widget

use crate::graphics::Rect;
use serde::{Deserialize, Serialize};

/// Form field export format types matching Datalogics AcroFormExportType.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AcroFormExportType {
    #[default]
    FDF = 1,
    XFDF = 2,
    XML = 3,
    JSON = 4,
}

/// Form field import format types matching Datalogics AcroFormImportType.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AcroFormImportType {
    #[default]
    FDF = 1,
    XFDF = 2,
    XML = 3,
    JSON = 4,
}

/// Text input field properties.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TextField {
    pub multiline: bool,
    pub password: bool,
    pub max_len: Option<usize>,
}

/// Button, checkbox, or radio field properties.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ButtonField {
    pub is_checkbox: bool,
    pub is_radio: bool,
    pub is_push_button: bool,
    pub checked: bool,
    pub export_value: String,
}

/// Dropdown combo or list selection field properties.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ChoiceField {
    pub options: Vec<String>,
    pub multi_select: bool,
    pub editable: bool,
}

/// Signature form field widget properties.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SignatureField {
    pub is_signed: bool,
    pub signer_name: Option<String>,
}

/// Specific form field type variant payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldType {
    Text(TextField),
    Button(ButtonField),
    Choice(ChoiceField),
    Signature(SignatureField),
}

/// Interactive AcroForm field representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub full_name: String,
    pub value: Option<String>,
    pub rect: Rect,
    pub page_number: usize,
    pub read_only: bool,
    pub required: bool,
    pub field_type: FieldType,
}

impl Field {
    pub fn new_text(name: impl Into<String>, page_number: usize, rect: Rect) -> Self {
        let n = name.into();
        Self {
            full_name: n.clone(),
            name: n,
            value: None,
            rect,
            page_number,
            read_only: false,
            required: false,
            field_type: FieldType::Text(TextField::default()),
        }
    }
}
