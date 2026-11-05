//! Popups and floating windows. The anchored toolbar dropdowns are in `dropdowns`, their shared shell in
//! `popup_frame`, and the rest by name: `menus`, `confirm`, `find_bar`, `shortcuts_help`.

use super::*;
use crate::store::model::Kind;
use crate::trail::history::Dir;

mod confirm;
mod dropdowns;
mod find_bar;
mod menus;
mod popup_frame;
mod shortcuts_help;

pub(crate) use confirm::*;
#[cfg(test)] // the trail popup's tests (`trail::nav`) place it by `popup_start_rect`
pub(crate) use dropdowns::*;
pub(crate) use find_bar::*;
pub(crate) use menus::*;
pub(crate) use popup_frame::*;

/// A `TextEdit` that a shortcut or click wants focused. The widget may not
/// exist until a later frame (a rename field replaces its button, the find
/// bar opens), so the request waits in `App::pending_focus` until the
/// widget is built and takes it with `take_focus`. One at a time, since
/// only one widget can hold focus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FocusTarget {
    /// The inline omnibar (`App::omni`), whichever view shows it.
    OmniBar,
    /// The Ctrl+K popup's omnibar (`App::omni_popup`).
    OmniPopup,
    FindBar,
    GroupRename,
    BookmarkFolder,
    /// The tab menu's "New Jar..." name field (`App::naming_jar`).
    JarName,
}

/// True, once, if `target` is the pending focus request: clears it so the
/// caller can `request_focus()` its widget.
pub(crate) fn take_focus(pending: &mut Option<FocusTarget>, target: FocusTarget) -> bool {
    if *pending == Some(target) {
        *pending = None;
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_focus_request_is_taken_once_by_its_own_widget() {
        let mut pending = Some(FocusTarget::FindBar);
        assert!(!take_focus(&mut pending, FocusTarget::OmniBar));
        assert!(take_focus(&mut pending, FocusTarget::FindBar));
        assert!(!take_focus(&mut pending, FocusTarget::FindBar));
    }
}

#[cfg(test)]
mod view_picker_tests;
