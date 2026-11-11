//! The scanned pages of the document being put together, in the order they will be saved.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use scan_core::Page;
use serde::{Deserialize, Serialize};

/// A page's identity for the UI; never reused while the app runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PageId(u64);

impl fmt::Display for PageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl PageId {
    /// The id in a `page://localhost/<id>` path, such as `"/3"`; `None` for anything else.
    #[must_use]
    pub fn from_path(path: &str) -> Option<Self> {
        let digits = path.strip_prefix('/')?;
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok().map(Self)
    }
}

/// What the UI shows about a page; serialised as `{id, width_px, height_px, dpi, color}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PageMeta {
    /// The page's id.
    pub id: PageId,
    /// Width in pixels.
    pub width_px: u32,
    /// Height in pixels.
    pub height_px: u32,
    /// Resolution it was scanned at.
    pub dpi: u16,
    /// True for a colour page, false for grey.
    pub color: bool,
}

/// Why the page list could not be changed. Lowercase without a final period, like
/// [`scan_core::ScanError`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No page has this id.
    #[error("page {0} is no longer in the document; it may have been deleted already")]
    UnknownPage(PageId),
    /// The new order leaves out, repeats or invents a page.
    #[error("the new page order must list every page exactly once; try moving the page again")]
    NotAPermutation,
}

/// The pages in document order, and whether they changed since the last save.
#[derive(Debug, Default)]
pub struct PageStore {
    pages: Vec<(PageId, Arc<Page>)>,
    next_id: u64,
    revision: u64,
    saved_revision: u64,
}

impl PageStore {
    /// An empty document.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a page and returns what the UI shows of it.
    pub fn add(&mut self, page: Page) -> PageMeta {
        let id = PageId(self.next_id);
        self.next_id += 1;
        let meta = meta(id, &page);
        self.pages.push((id, Arc::new(page)));
        self.changed();
        meta
    }

    /// Removes a page.
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownPage`] when no page has this id.
    pub fn delete(&mut self, id: PageId) -> Result<(), StoreError> {
        let index = self
            .pages
            .iter()
            .position(|(page_id, _)| *page_id == id)
            .ok_or(StoreError::UnknownPage(id))?;
        self.pages.remove(index);
        self.changed();
        Ok(())
    }

    /// Puts the pages in the order of `ids`.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotAPermutation`] unless `ids` lists every page exactly once; the order is
    /// then unchanged.
    pub fn reorder(&mut self, ids: &[PageId]) -> Result<(), StoreError> {
        let unique: HashSet<PageId> = ids.iter().copied().collect();
        let same_pages = unique.len() == ids.len()
            && ids.len() == self.pages.len()
            && self.pages.iter().all(|(id, _)| unique.contains(id));
        if !same_pages {
            return Err(StoreError::NotAPermutation);
        }
        if self.pages.iter().map(|(id, _)| id).eq(ids) {
            return Ok(());
        }
        let mut pages = std::mem::take(&mut self.pages);
        for id in ids {
            if let Some(index) = pages.iter().position(|(page_id, _)| page_id == id) {
                self.pages.push(pages.swap_remove(index));
            }
        }
        self.changed();
        Ok(())
    }

    /// The pages in document order, shared so they can be written without holding the store.
    #[must_use]
    pub fn in_order(&self) -> Vec<Arc<Page>> {
        self.pages
            .iter()
            .map(|(_, page)| Arc::clone(page))
            .collect()
    }

    /// The page with this id.
    #[must_use]
    pub fn get(&self, id: PageId) -> Option<Arc<Page>> {
        self.pages
            .iter()
            .find(|(page_id, _)| *page_id == id)
            .map(|(_, page)| Arc::clone(page))
    }

    /// How many pages the document has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pages.len()
    }

    /// Whether the document has no pages.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// Counts changes, so a save can tell whether the pages changed while it wrote them.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether closing now would lose pages: there are pages, and they changed since the last
    /// save.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        !self.pages.is_empty() && self.revision != self.saved_revision
    }

    /// Records that the pages as they were at `revision` are saved. Changes made after that
    /// revision keep the document dirty, and a save that finishes after a later one does not
    /// undo it.
    pub fn mark_saved(&mut self, revision: u64) {
        self.saved_revision = self.saved_revision.max(revision);
    }

    fn changed(&mut self) {
        self.revision += 1;
    }
}

fn meta(id: PageId, page: &Page) -> PageMeta {
    let info = page.info();
    PageMeta {
        id,
        width_px: info.width,
        height_px: info.height,
        dpi: page.dpi(),
        color: info.components > 1,
    }
}

#[cfg(test)]
mod tests;
