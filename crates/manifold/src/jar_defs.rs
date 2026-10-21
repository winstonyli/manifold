//! Named cookie jars as synced definitions (`App::jar_defs`, spec "Data model"). A jar is a `Kind::Jar` card: a
//! `App::nodes` entry with a unit `Runtime::Jar`. Its fields are mirror state (2b-5 Ruling 7): written through
//! `App::edit_field` here and applied from the document by `materialize` like any card's.

use crate::caps::{Scope, Storage, JAR_EDGE};
use crate::jar::Jar;
use crate::node::Runtime;
use crate::popups::Confirm;
use crate::session_sync::color_value;
use crate::store::model::{field, FieldValue, Kind, NodeId};
use crate::theme::TAG_COLOR_PALETTE;
use crate::App;
use std::collections::BTreeMap;

/// A named jar's synced definition (a `Kind::Jar` card), decoded from its mirrored fields (`Mirror::jar_def`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JarDef {
    pub(crate) name: String,
    pub(crate) color: Option<egui::Color32>,
    pub(crate) ephemeral: bool,
    /// The built-in Default jar (`field::IS_DEFAULT`); exactly one live jar has it once `ensure_default_jar` has run.
    pub(crate) is_default: bool,
}

/// The Default jar's name when it is made, and the name a losing duplicate takes (spec "The Default jar").
const DEFAULT_NAME: &str = "Default";
const OLD_DEFAULT_NAME: &str = "Default (old)";

/// A jar action waiting for its confirm dialog (`App::build_confirm_popup`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JarConfirm {
    Clear(NodeId),
    /// Ephemeral turned on for a jar with a saved file (`App::request_jar_ephemeral`).
    Ephemeral(NodeId),
}

mod label;
mod lifecycle;
pub(crate) use label::*;

#[cfg(test)]
mod tests;
