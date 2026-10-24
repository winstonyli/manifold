//! The Settings page as data: one `SettingRow` per control group, in page order. The page draws the rows
//! (`App::build_settings_page`) and the omnibar searches them (`matching`), so a setting is findable by what it is
//! called and by the words people use for it.

use super::*;
use crate::settings_page::SettingsRequests;

pub(crate) struct SettingRow {
    pub(crate) id: &'static str,
    /// Rows with the same section, side by side in the table, share one heading on the page.
    pub(crate) section: &'static str,
    pub(crate) title: &'static str,
    /// Other words for the row's controls, lowercase, matched like the title.
    pub(crate) keywords: &'static str,
    pub(crate) draw: fn(&mut App, &mut egui::Ui, &mut SettingsRequests),
}

pub(crate) fn rows() -> &'static [SettingRow] {
    &ROWS
}

/// The rows whose title and keywords hold every word of `query` as a substring, in page order. An empty query
/// matches nothing.
pub(crate) fn matching(query: &str) -> Vec<&'static SettingRow> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    rows()
        .iter()
        .filter(|row| {
            let text = format!("{} {}", row.title, row.keywords).to_lowercase();
            words.iter().all(|word| text.contains(word.as_str()))
        })
        .collect()
}

static ROWS: [SettingRow; 13] = [
    SettingRow {
        id: "welcome_tour",
        section: "Help",
        title: "Welcome tour",
        keywords: "first launch tutorial guide intro help",
        draw: |app, ui, req| app.settings_welcome_tour_row(ui, req),
    },
    SettingRow {
        id: "theme",
        section: "General",
        title: "Theme",
        keywords: "dark light colour color appearance night",
        draw: |app, ui, _| app.settings_theme_row(ui),
    },
    SettingRow {
        id: "search_engine",
        section: "General",
        title: "Default search engine",
        keywords: "search duckduckgo google bing suggestions address bar",
        draw: |app, ui, _| app.settings_search_engine_row(ui),
    },
    SettingRow {
        id: "starting_engine",
        section: "General",
        title: "Default starting engine",
        keywords: "new tabs blink webview2 servo webkit cef renderer",
        draw: |app, ui, req| app.settings_starting_engine_row(ui, req),
    },
    SettingRow {
        id: "zoom",
        section: "General",
        title: "Zoom",
        keywords: "magnify scale page size text bigger smaller",
        draw: |app, ui, _| app.settings_zoom_row(ui),
    },
    SettingRow {
        id: "home_page",
        section: "General",
        title: "Home page",
        keywords: "new tabs start url address",
        draw: |app, ui, _| app.settings_home_page_row(ui),
    },
    SettingRow {
        id: "sleep_tabs",
        section: "General",
        title: "Sleep background tabs",
        keywords: "idle memory minutes pinned audio unload",
        draw: |app, ui, _| app.settings_sleep_row(ui),
    },
    SettingRow {
        id: "current_tab",
        section: "Current Tab",
        title: "Current tab",
        keywords: "engine mute sound audio screenshot picture print pdf",
        draw: |app, ui, req| app.settings_current_tab_row(ui, req),
    },
    SettingRow {
        id: "engines",
        section: "Engines",
        title: "Engines",
        keywords: "download install update remove retry blink cef webview2 servo webkit folder disk",
        draw: |app, ui, _| app.settings_view_kinds_row(ui),
    },
    SettingRow {
        id: "lists",
        section: "Lists",
        title: "Bookmarks, history and downloads",
        keywords: "lists visited pages saved files",
        draw: |app, ui, req| app.settings_lists_row(ui, req),
    },
    SettingRow {
        id: "sessions",
        section: "Sessions",
        title: "Sessions",
        keywords: "save restore open delete export import backup autosave",
        draw: |app, ui, req| app.settings_sessions_row(ui, req),
    },
    SettingRow {
        id: "bookmark_files",
        section: "Bookmarks",
        title: "Export and import bookmarks",
        keywords: "file backup html favourites favorites",
        draw: |app, ui, req| app.settings_bookmarks_row(ui, req),
    },
    SettingRow {
        id: "search_index",
        section: "Search",
        title: "Clear search index",
        keywords: "privacy history text forget delete page content",
        draw: |app, ui, req| app.settings_search_index_row(ui, req),
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn ids(query: &str) -> Vec<&'static str> {
        matching(query).iter().map(|r| r.id).collect()
    }

    #[test]
    fn row_ids_are_unique_and_titled() {
        let mut seen = HashSet::new();
        for row in rows() {
            assert!(seen.insert(row.id), "{} appears twice", row.id);
            assert!(!row.title.is_empty() && !row.section.is_empty(), "{} needs a title and a section", row.id);
        }
        assert!(rows().len() >= 10, "the page has {} rows", rows().len());
    }

    #[test]
    fn each_section_is_one_run_of_rows() {
        let mut closed = HashSet::new();
        let mut current = "";
        for row in rows() {
            if row.section != current {
                assert!(closed.insert(current), "section {current} is split by another");
                current = row.section;
            }
        }
    }

    #[test]
    fn every_row_is_found_by_its_title_in_any_case() {
        for row in rows() {
            assert!(ids(&row.title.to_lowercase()).contains(&row.id), "{}", row.title);
            assert!(ids(&row.title.to_uppercase()).contains(&row.id), "{}", row.title);
        }
    }

    #[test]
    fn keywords_match_but_unrelated_text_does_not() {
        assert_eq!(ids("dark"), ["theme"]);
        assert_eq!(ids("search engine"), ["search_engine"]);
        assert!(ids("mute").contains(&"current_tab"));
        assert!(ids("zzzz").is_empty());
        assert!(ids("   ").is_empty());
    }
}
