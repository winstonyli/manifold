//! Find-in-page via WebView2's real native `ICoreWebView2Find` API
//! (`ICoreWebView2_28::Find`, `ICoreWebView2Environment15::CreateFindOptions`)
//! -- unlike click/scroll/text entry (`input.rs`), this isn't
//! `ExecuteScript`-driven: WebView2 exposes the browser's own built-in find
//! engine directly (the same one behind a real Edge window's Ctrl+F), match
//! highlighting included, so there's no reason to reimplement it in JS.
//! `SuppressDefaultFindDialog` keeps Edge's own find bar UI from ever
//! appearing, since `manifold` builds its own.

use std::cell::RefCell;
use std::rc::Rc;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Environment, ICoreWebView2Environment15, ICoreWebView2Find, ICoreWebView2FindOptions,
    ICoreWebView2_28,
};
use webview2_com::{
    FindActiveMatchIndexChangedEventHandler, FindMatchCountChangedEventHandler, FindStartCompletedHandler,
};
use windows::core::HSTRING;

/// `(count, active_index)` — mirrors `ipc_protocol::EngineToHost::FindResult`
/// exactly, so `main.rs`'s tick loop can forward it unchanged.
pub type PendingFindResult = Rc<RefCell<Option<(u32, u32)>>>;

pub struct FindState {
    find: ICoreWebView2Find,
    /// Reused across every `start()` call (only `FindTerm`/`IsCaseSensitive`
    /// change) rather than recreated per search — `CreateFindOptions` needs
    /// the environment, which `start()` itself has no reason to hold onto.
    options: ICoreWebView2FindOptions,
    pending: PendingFindResult,
}

fn read_result(find: &ICoreWebView2Find) -> (u32, u32) {
    let mut count: i32 = 0;
    let mut active: i32 = 0;
    unsafe {
        let _ = find.MatchCount(&mut count);
        let _ = find.ActiveMatchIndex(&mut active);
    }
    (count.max(0) as u32, active.max(0) as u32)
}

/// Casts `webview`/`environment` up to the versioned interfaces find needs
/// and wires both result-change events into `pending`, drained by `main.rs`'s
/// tick loop the same way `favicon::register`'s result is. Best-effort: an
/// `ICoreWebView2_28`/`ICoreWebView2Environment15`-less WebView2 Runtime just
/// means this tab's find bar silently never reports results, same
/// graceful-degradation shape as favicon's own version-gated API.
pub fn register(environment: &ICoreWebView2Environment, webview: &ICoreWebView2) -> windows::core::Result<FindState> {
    let environment15: ICoreWebView2Environment15 = windows::core::Interface::cast(environment)?;
    let webview28: ICoreWebView2_28 = windows::core::Interface::cast(webview)?;
    let find = unsafe { webview28.Find()? };
    let options = unsafe { environment15.CreateFindOptions()? };
    unsafe {
        let _ = options.SetShouldHighlightAllMatches(true);
        let _ = options.SetSuppressDefaultFindDialog(true);
    }

    let pending: PendingFindResult = Rc::new(RefCell::new(None));
    let mut token = Default::default();

    let pending_for_count = pending.clone();
    let count_handler = FindMatchCountChangedEventHandler::create(Box::new(move |sender, _args| {
        if let Some(sender) = &sender {
            *pending_for_count.borrow_mut() = Some(read_result(sender));
        }
        Ok(())
    }));
    unsafe {
        let _ = find.add_MatchCountChanged(&count_handler, &mut token);
    }

    let pending_for_active = pending.clone();
    let active_handler = FindActiveMatchIndexChangedEventHandler::create(Box::new(move |sender, _args| {
        if let Some(sender) = &sender {
            *pending_for_active.borrow_mut() = Some(read_result(sender));
        }
        Ok(())
    }));
    unsafe {
        let _ = find.add_ActiveMatchIndexChanged(&active_handler, &mut token);
    }

    Ok(FindState { find, options, pending })
}

impl FindState {
    /// Begins (or replaces) a search for `text`. `manifold` sends this on
    /// every find-bar text change, not just on open — matches how a real
    /// browser's find bar re-searches as you type.
    pub fn start(&self, text: &str, match_case: bool) {
        unsafe {
            let _ = self.options.SetFindTerm(&HSTRING::from(text));
            let _ = self.options.SetIsCaseSensitive(match_case);
            let handler = FindStartCompletedHandler::create(Box::new(|_error| Ok(())));
            let _ = self.find.Start(&self.options, &handler);
        }
    }

    pub fn next(&self) {
        unsafe {
            let _ = self.find.FindNext();
        }
    }

    pub fn previous(&self) {
        unsafe {
            let _ = self.find.FindPrevious();
        }
    }

    pub fn stop(&self) {
        unsafe {
            let _ = self.find.Stop();
        }
    }

    pub fn pending(&self) -> &PendingFindResult {
        &self.pending
    }
}
