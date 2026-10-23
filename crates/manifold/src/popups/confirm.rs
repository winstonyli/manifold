//! The confirm dialogs: what each asks, the dialog itself, and the one slot that holds the open confirm (`App::confirm`).

use super::*;

/// A centred confirm dialog: `title`, then `detail` (the first line is the explanation, any later line a warning, as
/// `download_detail` writes), then `button` (filled: rust when `destructive`, else gold and focused, so Enter confirms) and Cancel. `Some(true)` on
/// the button, `Some(false)` on Cancel or Escape, `None` while it waits.
fn confirm_dialog(
    ui: &mut egui::Ui,
    area_id: &str,
    title: String,
    detail: &str,
    button: &str,
    destructive: bool,
) -> Option<bool> {
    let (mut yes, mut no) = (false, false);
    egui::Area::new(egui::Id::new(area_id))
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            dialog_frame(ui).show(ui, |ui| {
                control_font(ui);
                ui.set_min_width(240.0);
                ui.label(heading_text(title));
                ui.add_space(4.0);
                for (i, line) in detail.lines().enumerate() {
                    let text = egui::RichText::new(line).small();
                    let text = if i == 0 { text.weak() } else { text.color(crate::theme::accent(ui.visuals())) };
                    ui.add(egui::Label::new(text).wrap());
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let main = if destructive { danger_button(ui, button) } else { primary_button(ui, button) };
                    // A safe action opens with its button focused, so Enter confirms it. A destructive one opens
                    // with nothing focused: Enter does nothing, Tab reaches the buttons.
                    if popup_just_opened(ui.ctx(), egui::Id::new((area_id, "just_opened")), true) && !destructive {
                        main.request_focus();
                    }
                    yes = main.clicked();
                    no = ui.button("Cancel").clicked();
                });
            });
        });
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        no = true;
    }
    if yes {
        Some(true)
    } else if no {
        Some(false)
    } else {
        None
    }
}

/// The one confirm dialog that may be up (`App::confirm`). `confirm_dialog` is non-modal and centred, so a second
/// would replace the first unanswered or draw over it: `App::offer_confirm` ignores a click while one is up.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Confirm {
    /// An engine download or removal.
    View(ViewConfirm),
    /// A Settings jar action that signs tabs out or loses a jar or its logins.
    Jar(crate::jar_defs::JarConfirm),
    /// A named session's delete from Settings.
    SessionDelete(String),
    /// A picked session file staged for "Import Session..." (or headless `IMPORTSESSION`): shows its tab count
    /// against the running session's before replacing it, so a wrong file is a deliberate second click.
    Import(Box<crate::store::json::JsonDoc>),
}

/// The download dialog's detail text, with a warning line when system libraries the host needs are missing.
fn download_detail(kind: ViewKind, missing: &[&str]) -> String {
    let mut detail = "Downloaded once, checked against a pinned hash, then kept in your profile.".to_string();
    detail.push_str(kind.desc().download_note);
    if !missing.is_empty() {
        let fix = if cfg!(windows) {
            "Install the Microsoft Visual C++ Redistributable (x64)"
        } else {
            "Install them with your package manager"
        };
        detail.push_str(&format!(
            "
Missing system libraries: {}. {fix}, or {} will not start.",
            missing.join(", "),
            view_label(kind)
        ));
    }
    detail
}

/// "Download Servo, 93 MB?" (`size` from `view_info::megabytes`).
fn download_confirm_title(verb: &str, kind: ViewKind, size: &str) -> String {
    format!("{verb} {}, {size}?", view_label(kind))
}

impl App {
    /// Opens `confirm` unless a confirm is already up. Returns whether it opened.
    pub(crate) fn offer_confirm(&mut self, confirm: Confirm) -> bool {
        let free = self.confirm.is_none();
        if free {
            self.confirm = Some(confirm);
        }
        free
    }

    /// Whether a confirm dialog is up (or the import confirm): it owns Escape and the keyboard.
    pub(crate) fn confirm_open(&self) -> bool {
        self.confirm.is_some()
    }

    /// The up confirm's dialog: title, detail, button and whether it is destructive, or `None` when it no longer
    /// applies (a jar deleted on another device meanwhile just drops the question).
    fn confirm_text(&self, confirm: &Confirm) -> Option<(String, String, &'static str, bool)> {
        Some(match confirm {
            Confirm::SessionDelete(name) => {
                let shown = crate::names::to_plain(&crate::names::display_name(name, crate::names::UNTITLED));
                (
                    format!("Delete session {shown}?"),
                    "Deletes the saved session and its copies from disk.".to_string(),
                    "Delete",
                    true,
                )
            }
            Confirm::View(ViewConfirm::Download(kind)) => {
                let kind = *kind;
                let size = crate::view_info::megabytes(self.view_download_size(kind));
                let verb = if matches!(self.view_status(kind), crate::view_install::ViewStatus::UpdateNeeded { .. }) {
                    "Update"
                } else {
                    "Download"
                };
                let detail = download_detail(kind, &crate::view_install::missing_system_libs(kind));
                (download_confirm_title(verb, kind, &size), detail, verb, false)
            }
            Confirm::View(ViewConfirm::Remove(kind)) if crate::view_install::installed_paths(*kind).is_empty() => (
                format!("Clear {} leftovers?", view_label(*kind)),
                "Deletes the partial download and install leftovers.".to_string(),
                "Clear",
                true,
            ),
            Confirm::View(ViewConfirm::Remove(kind)) => (
                format!("Remove {}?", view_label(*kind)),
                "Deletes the downloaded files. You can download them again later.".to_string(),
                "Remove",
                true,
            ),
            Confirm::Import(json) => {
                let tabs = |n: usize| format!("{n} tab{}", if n == 1 { "" } else { "s" });
                let incoming = json.nodes.iter().filter(|n| n.kind() == Some(Kind::Tab)).count();
                (
                    "Import Session?".to_string(),
                    format!(
                        "Current session: {}. Importing: {}.
Importing replaces every currently open tab.",
                        tabs(self.nodes.len()),
                        tabs(incoming)
                    ),
                    "Import",
                    true,
                )
            }
            Confirm::Jar(jar) => {
                let (title, detail, button) = self.jar_confirm_text(*jar)?;
                (title, detail, button, true)
            }
        })
    }

    /// The confirm before a large or destructive action ("Download CEF, 172 MB?", "Delete session Work?", "Clear
    /// cookies in Work?"): asks, then runs it on Yes.
    pub(crate) fn build_confirm_popup(&mut self, ui: &mut egui::Ui) {
        let Some(confirm) = self.confirm.clone() else { return };
        let Some((title, detail, button, destructive)) = self.confirm_text(&confirm) else {
            self.confirm = None;
            return;
        };
        match confirm_dialog(ui, "confirm_popup", title, &detail, button, destructive) {
            Some(true) => {
                self.confirm = None;
                match confirm {
                    Confirm::View(ViewConfirm::Download(kind)) => {
                        self.start_view_install(kind);
                    }
                    Confirm::View(ViewConfirm::Remove(kind)) => self.remove_view(kind),
                    Confirm::Jar(crate::jar_defs::JarConfirm::Clear(key)) => self.clear_jar_cookies(key),
                    Confirm::Jar(crate::jar_defs::JarConfirm::Ephemeral(id)) => {
                        self.set_jar_ephemeral(id, true);
                        self.save_session();
                    }
                    Confirm::Import(json) => {
                        self.replace_running_session(&json, crate::store::json::Trust::Foreign);
                    }
                    Confirm::SessionDelete(name) => {
                        session::delete_named(&name);
                        crate::settings_page::forget_disk_cache(ui.ctx());
                    }
                }
            }
            Some(false) => self.confirm = None,
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_download_confirm_names_the_view_and_its_size() {
        use crate::process_manager::ViewKind;
        let size = crate::view_info::megabytes(93_000_000);
        assert_eq!(super::download_confirm_title("Download", ViewKind::Servo, &size), "Download Servo, 93 MB?");
    }

    /// The confirm's main button is filled rust when it destroys something and gold when it doesn't, and only the
    /// gold one opens focused (so Enter confirms it).
    #[test]
    fn a_confirm_fills_its_main_button_by_what_it_does() {
        let fills = |destructive: bool| {
            let ctx = crate::theme::testutil::ctx(true);
            let input = |frame: usize| egui::RawInput {
                time: Some(frame as f64),
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0))),
                ..Default::default()
            };
            let mut shapes = Vec::new();
            for frame in 0..4 {
                let mut out = ctx.run_ui(input(frame), |ui| {
                    super::confirm_dialog(ui, "t", "Title".into(), "Detail", "Go", destructive);
                });
                out.textures_delta.clear();
                shapes = out.shapes;
            }
            let mut colors = Vec::new();
            for clipped in &shapes {
                if let egui::Shape::Rect(rect) = &clipped.shape {
                    colors.push(rect.fill);
                }
            }
            (colors, ctx.memory(|m| m.focused().is_some()))
        };
        let ((rust, rust_focused), (gold, gold_focused)) = (fills(true), fills(false));
        assert!(gold_focused && !rust_focused, "focus: gold {gold_focused}, rust {rust_focused}");
        assert!(rust.contains(&crate::theme::DANGER_COLOR) && !rust.contains(&crate::theme::ACCENT), "{rust:?}");
        assert!(gold.contains(&crate::theme::ACCENT) && !gold.contains(&crate::theme::DANGER_COLOR), "{gold:?}");
    }

    #[test]
    fn download_detail_warns_only_when_libraries_are_missing() {
        assert!(!super::download_detail(ViewKind::Cef, &[]).contains("Missing"));
        let d = super::download_detail(ViewKind::Cef, &["libnss3.so", "libnspr4.so"]);
        assert!(d.contains("Missing system libraries: libnss3.so, libnspr4.so."), "{d}");
        assert!(d.ends_with("Blink (CEF) will not start."), "{d}");
    }

    #[test]
    fn the_webkit_download_says_what_wsl_needs_and_the_others_do_not() {
        let d = super::download_detail(ViewKind::Webkit, &[]);
        assert!(
            d.contains("Ubuntu") && d.contains("xvfb-run") && d.contains("WebKitGTK") && d.contains("Not included")
        );
        assert!(!super::download_detail(ViewKind::Servo, &[]).contains("WSL"));
    }

    /// A confirm opens only while none is up; it never replaces one.
    #[test]
    fn a_confirm_opens_only_while_none_is_up() {
        let mut app = App::default();
        let jar = crate::jar_defs::JarConfirm::Clear(crate::store::model::NodeId {
            actor: crate::store::model::ActorId(1),
            counter: 1,
        });
        assert!(app.offer_confirm(Confirm::Jar(jar)));
        assert!(!app.offer_confirm(Confirm::SessionDelete("work".into())), "another kind is up");
        assert!(!app.offer_confirm(Confirm::Jar(jar)), "its own kind is up");
        assert_eq!(app.confirm, Some(Confirm::Jar(jar)));
        app.confirm = None;
        assert!(app.offer_confirm(Confirm::SessionDelete("work".into())));
    }

    /// A staged session import is a confirm like the rest: it owns the keyboard and a second one is refused.
    #[test]
    fn a_staged_import_is_a_confirm() {
        let mut app = App::default();
        assert!(!app.modal_open());
        assert!(app.offer_confirm(Confirm::Import(Box::default())));
        assert!(app.modal_open() && app.confirm_open());
        assert!(!app.offer_confirm(Confirm::SessionDelete("work".into())));
        let (title, detail, button, destructive) = app.confirm_text(&Confirm::Import(Box::default())).unwrap();
        assert_eq!((title.as_str(), button, destructive), ("Import Session?", "Import", true));
        assert!(detail.contains("Importing: 0 tabs"), "{detail}");
    }

    /// Escape on any confirm answers the confirm only: Settings stays open behind it.
    #[test]
    fn escape_on_a_confirm_keeps_settings_open() {
        let ctx = crate::theme::testutil::ctx(true);
        for confirm in [Confirm::SessionDelete("work".into()), Confirm::View(ViewConfirm::Remove(ViewKind::Servo))] {
            let mut app = App { view_mode: ViewMode::Settings, confirm: Some(confirm), ..App::default() };
            let key = egui::Key::Escape;
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 1000.0))),
                events: vec![egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            };
            let mut full = ctx.run_ui(raw, |ui| app.build_settings_page(ui));
            full.textures_delta.clear();
            assert_eq!(app.view_mode, ViewMode::Settings, "{:?}", app.confirm);
        }
    }
}
