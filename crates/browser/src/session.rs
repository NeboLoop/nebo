use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::{BrowserError, ConsoleMessage, ElementRef, PageError};

/// Browser session for a profile.
pub struct Session {
    pub profile_name: String,
    pub cdp_url: String,
    pages: RwLock<HashMap<String, Arc<Page>>>,
    active_target: RwLock<Option<String>>,
}

impl Session {
    pub fn new(profile_name: &str, cdp_url: &str) -> Self {
        Self {
            profile_name: profile_name.to_string(),
            cdp_url: cdp_url.to_string(),
            pages: RwLock::new(HashMap::new()),
            active_target: RwLock::new(None),
        }
    }

    /// Add a page to this session.
    pub fn add_page(&self, target_id: &str, page: Page) -> Arc<Page> {
        let page = Arc::new(page);
        let mut pages = self.pages.write().unwrap();
        pages.insert(target_id.to_string(), page.clone());

        let mut active = self.active_target.write().unwrap();
        if active.is_none() {
            *active = Some(target_id.to_string());
        }

        page
    }

    /// Get the active page.
    pub fn active_page(&self) -> Result<Arc<Page>, BrowserError> {
        let active = self.active_target.read().unwrap();
        let target_id = active
            .as_ref()
            .ok_or_else(|| BrowserError::PageNotFound("no active page".into()))?;
        let pages = self.pages.read().unwrap();
        pages
            .get(target_id)
            .cloned()
            .ok_or_else(|| BrowserError::PageNotFound(target_id.clone()))
    }

    /// Set the active page target.
    pub fn set_active(&self, target_id: &str) -> Result<(), BrowserError> {
        let pages = self.pages.read().unwrap();
        if !pages.contains_key(target_id) {
            return Err(BrowserError::PageNotFound(target_id.into()));
        }
        drop(pages);
        let mut active = self.active_target.write().unwrap();
        *active = Some(target_id.to_string());
        Ok(())
    }

    /// Get a page by target ID.
    pub fn get_page(&self, target_id: &str) -> Option<Arc<Page>> {
        self.pages.read().unwrap().get(target_id).cloned()
    }

    /// Remove a page.
    pub fn remove_page(&self, target_id: &str) {
        let mut pages = self.pages.write().unwrap();
        pages.remove(target_id);
        let mut active = self.active_target.write().unwrap();
        if active.as_deref() == Some(target_id) {
            *active = pages.keys().next().cloned();
        }
    }

    /// List all page target IDs.
    pub fn page_ids(&self) -> Vec<String> {
        self.pages.read().unwrap().keys().cloned().collect()
    }

    /// Number of open pages.
    pub fn page_count(&self) -> usize {
        self.pages.read().unwrap().len()
    }
}

/// A browser page with state tracking.
pub struct Page {
    pub target_id: String,
    state: RwLock<PageState>,
    refs: RwLock<Vec<ElementRef>>,
}

/// Current state of a page.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageState {
    pub url: String,
    pub title: String,
    pub console_messages: Vec<ConsoleMessage>,
    pub errors: Vec<PageError>,
}

impl Page {
    pub fn new(target_id: &str) -> Self {
        Self {
            target_id: target_id.to_string(),
            state: RwLock::new(PageState::default()),
            refs: RwLock::new(Vec::new()),
        }
    }

    /// Get current page state.
    pub fn state(&self) -> PageState {
        self.state.read().unwrap().clone()
    }

    /// Update the page URL and title.
    pub fn update_state(&self, url: &str, title: &str) {
        let mut state = self.state.write().unwrap();
        state.url = url.to_string();
        state.title = title.to_string();
    }

    /// Add a console message (keeps last 100).
    pub fn add_console_message(&self, msg: ConsoleMessage) {
        let mut state = self.state.write().unwrap();
        state.console_messages.push(msg);
        if state.console_messages.len() > 100 {
            state.console_messages.remove(0);
        }
    }

    /// Add a page error (keeps last 50).
    pub fn add_error(&self, err: PageError) {
        let mut state = self.state.write().unwrap();
        state.errors.push(err);
        if state.errors.len() > 50 {
            state.errors.remove(0);
        }
    }

    /// Set element refs (from accessibility snapshot).
    pub fn set_refs(&self, refs: Vec<ElementRef>) {
        let mut r = self.refs.write().unwrap();
        *r = refs;
    }

    /// Get element refs.
    pub fn get_refs(&self) -> Vec<ElementRef> {
        self.refs.read().unwrap().clone()
    }

    /// Clear element refs (e.g. on navigation).
    pub fn clear_refs(&self) {
        let mut r = self.refs.write().unwrap();
        r.clear();
    }

    /// Resolve a ref like "e1" to its selector, or return the input as-is.
    pub fn resolve_selector(&self, input: &str) -> String {
        if input.starts_with('e') {
            let refs = self.refs.read().unwrap();
            for r in refs.iter() {
                if r.id == input {
                    return r.selector.clone();
                }
            }
        }
        input.to_string()
    }

    /// Get console messages.
    pub fn console_messages(&self) -> Vec<ConsoleMessage> {
        self.state.read().unwrap().console_messages.clone()
    }

    /// Get page errors.
    pub fn errors(&self) -> Vec<PageError> {
        self.state.read().unwrap().errors.clone()
    }
}
