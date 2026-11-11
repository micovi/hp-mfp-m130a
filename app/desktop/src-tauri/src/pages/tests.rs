use scan_core::Page;
use scan_core::testing::fixture;

use super::*;

fn grey() -> Page {
    grey_at(75)
}

fn grey_at(dpi: u16) -> Page {
    Page::new(fixture("synthetic-grey-75dpi.jpg").unwrap(), dpi).unwrap()
}

fn colour() -> Page {
    Page::new(fixture("synthetic-colour-150dpi.jpg").unwrap(), 150).unwrap()
}

/// A store holding pages at 75, 150 and 300 dpi, in that order, so `dpis` tells them apart.
fn three_pages() -> (PageStore, Vec<PageId>) {
    let mut store = PageStore::new();
    let ids = vec![
        store.add(grey()).id,
        store.add(colour()).id,
        store.add(grey_at(300)).id,
    ];
    (store, ids)
}

fn dpis(store: &PageStore) -> Vec<u16> {
    store.in_order().iter().map(|page| page.dpi()).collect()
}

#[test]
fn a_new_store_is_empty_and_clean() {
    let store = PageStore::new();
    assert!(store.is_empty());
    assert_eq!(store.len(), 0);
    assert_eq!(store.in_order().len(), 0);
    assert!(!store.is_dirty());
}

#[test]
fn add_describes_the_page_and_appends_it() {
    let mut store = PageStore::new();
    let first = store.add(grey());
    let second = store.add(colour());
    assert_eq!(
        first,
        PageMeta {
            id: PageId(0),
            width_px: 620,
            height_px: 877,
            dpi: 75,
            color: false,
        }
    );
    assert_eq!(
        second,
        PageMeta {
            id: PageId(1),
            width_px: 1240,
            height_px: 1754,
            dpi: 150,
            color: true,
        }
    );
    assert_eq!(dpis(&store), [75, 150]);
    assert!(store.is_dirty());
}

#[test]
fn page_meta_serialises_with_the_field_names_the_ui_reads() {
    let meta = PageMeta {
        id: PageId(7),
        width_px: 620,
        height_px: 877,
        dpi: 75,
        color: false,
    };
    assert_eq!(
        serde_json::to_string(&meta).unwrap(),
        r#"{"id":7,"width_px":620,"height_px":877,"dpi":75,"color":false}"#
    );
}

#[test]
fn get_finds_pages_by_id() {
    let (store, ids) = three_pages();
    assert_eq!(store.get(ids[1]).map(|page| page.dpi()), Some(150));
    assert!(store.get(PageId(99)).is_none());
}

#[test]
fn delete_removes_only_that_page_and_ids_are_not_reused() {
    let (mut store, ids) = three_pages();
    store.delete(ids[1]).unwrap();
    assert_eq!(dpis(&store), [75, 300]);
    assert!(store.get(ids[1]).is_none());
    assert_eq!(store.add(colour()).id, PageId(3));
}

#[test]
fn deleting_an_unknown_page_is_an_error_and_changes_nothing() {
    let (mut store, ids) = three_pages();
    store.mark_saved(store.revision());
    store.delete(ids[0]).unwrap();
    assert_eq!(store.delete(ids[0]), Err(StoreError::UnknownPage(ids[0])));
    assert_eq!(store.len(), 2);
    assert_eq!(
        StoreError::UnknownPage(ids[0]).to_string(),
        "page 0 is no longer in the document; it may have been deleted already"
    );
}

#[test]
fn reorder_puts_pages_in_the_given_order() {
    let (mut store, ids) = three_pages();
    store.reorder(&[ids[1], ids[2], ids[0]]).unwrap();
    assert_eq!(dpis(&store), [150, 300, 75]);
    assert_eq!(store.get(ids[2]).map(|page| page.dpi()), Some(300));
}

#[test]
fn reorder_rejects_anything_but_a_permutation() {
    let (mut store, ids) = three_pages();
    let not_permutations: [&[PageId]; 5] = [
        &[],
        &[ids[0], ids[1]],
        &[ids[0], ids[1], ids[2], ids[2]],
        &[ids[0], ids[1], ids[1]],
        &[ids[0], ids[1], PageId(42)],
    ];
    for ids in not_permutations {
        assert_eq!(
            store.reorder(ids),
            Err(StoreError::NotAPermutation),
            "{ids:?}"
        );
    }
    assert_eq!(dpis(&store), [75, 150, 300]);
}

#[test]
fn reordering_an_empty_store_to_nothing_is_fine() {
    let mut store = PageStore::new();
    assert_eq!(store.reorder(&[]), Ok(()));
}

#[test]
fn saving_clears_dirty_until_the_next_change() {
    let (mut store, ids) = three_pages();
    store.mark_saved(store.revision());
    assert!(!store.is_dirty());

    store.reorder(&[ids[2], ids[1], ids[0]]).unwrap();
    assert!(store.is_dirty());
    store.mark_saved(store.revision());

    store.delete(ids[1]).unwrap();
    assert!(store.is_dirty());
    store.mark_saved(store.revision());

    store.add(grey());
    assert!(store.is_dirty());
}

#[test]
fn reordering_to_the_same_order_or_failing_keeps_the_document_clean() {
    let (mut store, ids) = three_pages();
    store.mark_saved(store.revision());
    store.reorder(&ids).unwrap();
    assert!(store.reorder(&[ids[0]]).is_err());
    assert!(!store.is_dirty());
}

#[test]
fn a_change_made_while_saving_keeps_the_document_dirty() {
    let (mut store, _) = three_pages();
    let saving = store.revision();
    store.add(colour());
    store.mark_saved(saving);
    assert!(store.is_dirty());
}

#[test]
fn a_save_that_finishes_after_a_later_one_keeps_the_later_revision() {
    let (mut store, _) = three_pages();
    let first = store.revision();
    store.add(colour());
    store.mark_saved(store.revision());
    store.mark_saved(first);
    assert!(!store.is_dirty());
}

#[test]
fn deleting_every_page_leaves_nothing_to_lose() {
    let (mut store, ids) = three_pages();
    for id in ids {
        store.delete(id).unwrap();
    }
    assert!(store.is_empty());
    assert!(!store.is_dirty());
}

#[test]
fn page_ids_are_read_from_uri_paths() {
    assert_eq!(PageId::from_path("/0"), Some(PageId(0)));
    assert_eq!(PageId::from_path("/42"), Some(PageId(42)));
    for path in [
        "",
        "/",
        "42",
        "/-1",
        "/+1",
        "/1/2",
        "/x",
        "/ 1",
        "/99999999999999999999999",
    ] {
        assert_eq!(PageId::from_path(path), None, "{path:?}");
    }
}
