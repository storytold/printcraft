//! optionalcontent.rs — Optional Content (Layers) domain models matching Datalogics APDFL.
//! =========================================================================================
//!
//! Purpose:
//!     Defines OptionalContentGroup (OCG), OptionalContentContext, and OptionalContentConfig
//!     matching Datalogics APDFL Optional Content (Layers) specifications.
//!
//! Layer:
//!     Domain Layer / Optional Content (Layers)
//!
//! Key Types:
//!     - OptionalContentGroup: Individual layer with name and default visibility
//!     - OptionalContentContext: Runtime visibility evaluation context
//!     - OptionalContentConfig: Configuration preset controlling layer states

use serde::{Deserialize, Serialize};

/// An Optional Content Group (OCG) representing a visual layer in a PDF document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionalContentGroup {
    pub name: String,
    pub is_visible: bool,
    pub intent: Vec<String>,
}

impl OptionalContentGroup {
    pub fn new(name: impl Into<String>, is_visible: bool) -> Self {
        Self { name: name.into(), is_visible, intent: vec!["View".into(), "Design".into()] }
    }
}

/// Evaluation context controlling the dynamic visibility of layers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OptionalContentContext {
    pub active_groups: Vec<String>,
}

impl OptionalContentContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_group_visible(&mut self, group_name: impl Into<String>, visible: bool) {
        let name = group_name.into();
        if visible {
            if !self.active_groups.contains(&name) {
                self.active_groups.push(name);
            }
        } else {
            self.active_groups.retain(|g| g != &name);
        }
    }

    pub fn is_group_visible(&self, group_name: &str) -> bool {
        self.active_groups.iter().any(|g| g == group_name)
    }
}

/// Predefined layer configuration profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionalContentConfig {
    pub name: String,
    pub base_state: String,
    pub on_groups: Vec<String>,
    pub off_groups: Vec<String>,
}
