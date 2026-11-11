use std::sync::mpsc;
use std::time::{Duration, Instant};

use scan_core::testing::fixture;
use tauri::async_runtime::{block_on, spawn};

use super::*;

const GREY_300: ScanSettings = ScanSettings {
    color: ColorMode::Grey,
    dpi: 300,
};

fn grey_page(dpi: u16) -> Page {
    Page::new(fixture("synthetic-grey-75dpi.jpg").unwrap(), dpi).unwrap()
}

/// A desktop holding a 75 dpi and a 150 dpi page.
fn two_pages() -> (Desktop, Vec<PageId>) {
    let desktop = Desktop::new();
    let ids = [75, 150]
        .map(|dpi| {
            block_on(desktop.scan(GREY_300, move |_, _| Ok(grey_page(dpi))))
                .unwrap()
                .id
        })
        .to_vec();
    (desktop, ids)
}

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("scan-desktop-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn wait_until(condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn sentences_start_with_a_capital_and_end_with_a_period() {
    assert_eq!(
        sentence("the scanner is busy; try again"),
        "The scanner is busy; try again."
    );
    assert_eq!(sentence("already a sentence."), "Already a sentence.");
    assert_eq!(sentence("really?"), "Really?");
    assert_eq!(sentence("ünïcode"), "Ünïcode.");
    assert_eq!(sentence(""), ".");
}

#[test]
fn a_scan_passes_the_settings_and_adds_the_page() {
    let desktop = Desktop::new();
    let colour = ScanSettings {
        color: ColorMode::Colour,
        dpi: 150,
    };
    let meta = block_on(desktop.scan(colour, move |settings, _| {
        assert_eq!(settings, colour);
        Ok(grey_page(150))
    }))
    .unwrap();
    assert_eq!((meta.width_px, meta.height_px, meta.dpi), (620, 877, 150));
    assert!(desktop.page_jpeg(meta.id).unwrap().is_some());
    assert_eq!(desktop.has_unsaved_pages(), Ok(true));
}

#[test]
fn a_failed_scan_is_one_sentence_and_adds_nothing() {
    let desktop = Desktop::new();
    let result = block_on(desktop.scan(GREY_300, |_, _| Err(ScanError::ScannerBusy)));
    assert_eq!(
        result,
        Err("The scanner is busy; try again in a moment.".to_owned())
    );
    assert_eq!(desktop.has_unsaved_pages(), Ok(false));
}

#[test]
fn a_cancelled_scan_fails_with_the_cancel_sentence() {
    let desktop = Desktop::new();
    let result = block_on(desktop.scan(GREY_300, |_, _| Err(ScanError::Cancelled)));
    assert_eq!(
        result,
        Err("The scan was cancelled; start a new scan when ready.".to_owned())
    );
    assert_eq!(desktop.has_unsaved_pages(), Ok(false));
}

#[test]
fn cancel_stops_the_running_scan_and_the_next_scan_starts_uncancelled() {
    let desktop = Arc::new(Desktop::new());
    let running = Arc::clone(&desktop);
    let scan = spawn(async move {
        running
            .scan(GREY_300, |_, cancel| {
                while !cancel.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(ScanError::Cancelled)
            })
            .await
    });
    wait_until(|| desktop.scanning.load(Ordering::Acquire));
    desktop.cancel();
    assert!(block_on(scan).unwrap().is_err());

    let next = block_on(desktop.scan(GREY_300, |_, cancel| {
        assert!(!cancel.load(Ordering::Acquire));
        Ok(grey_page(300))
    }));
    assert!(next.is_ok());
}

#[test]
fn a_second_scan_is_refused_while_one_runs_and_allowed_after() {
    let desktop = Arc::new(Desktop::new());
    let (release, released) = mpsc::channel::<()>();
    let running = Arc::clone(&desktop);
    let first = spawn(async move {
        running
            .scan(GREY_300, move |_, _| {
                released.recv().unwrap();
                Ok(grey_page(300))
            })
            .await
    });
    wait_until(|| desktop.scanning.load(Ordering::Acquire));

    let second = block_on(desktop.scan(GREY_300, |_, _| Ok(grey_page(300))));
    assert_eq!(
        second,
        Err("A scan is already running; wait for it to finish or cancel it.".to_owned())
    );

    release.send(()).unwrap();
    assert!(block_on(first).unwrap().is_ok());
    assert!(block_on(desktop.scan(GREY_300, |_, _| Ok(grey_page(300)))).is_ok());
}

#[test]
fn a_failed_scan_frees_the_scanner_for_the_next() {
    let desktop = Desktop::new();
    assert!(block_on(desktop.scan(GREY_300, |_, _| Err(ScanError::Disconnected))).is_err());
    assert!(block_on(desktop.scan(GREY_300, |_, _| Ok(grey_page(300)))).is_ok());
}

#[test]
fn delete_and_reorder_report_sentences() {
    let (desktop, ids) = two_pages();
    desktop.reorder(&[ids[1], ids[0]]).unwrap();
    assert_eq!(
        desktop.reorder(&[ids[0]]),
        Err(
            "The new page order must list every page exactly once; try moving the page again."
                .to_owned()
        )
    );
    desktop.delete(ids[0]).unwrap();
    assert_eq!(
        desktop.delete(ids[0]),
        Err(format!(
            "Page {} is no longer in the document; it may have been deleted already.",
            ids[0]
        ))
    );
    assert_eq!(desktop.page_jpeg(ids[0]), Ok(None));
}

#[test]
fn save_writes_the_pages_in_order_and_clears_unsaved() {
    let (desktop, ids) = two_pages();
    desktop.reorder(&[ids[1], ids[0]]).unwrap();
    let path = scratch_dir("save").join("two pages.pdf");

    assert_eq!(block_on(desktop.save(path.clone())), Ok(2));

    let pages = desktop.pages().unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        write_pdf(&pages.in_order()).unwrap()
    );
    assert_eq!(pages.in_order()[0].dpi(), 150);
    assert!(!pages.is_dirty());
}

#[test]
fn saving_no_pages_is_refused() {
    let desktop = Desktop::new();
    let path = scratch_dir("empty").join("empty.pdf");
    assert_eq!(
        block_on(desktop.save(path.clone())),
        Err("There are no pages to save.".to_owned())
    );
    assert!(!path.exists());
}

#[test]
fn a_failed_write_names_the_path_and_keeps_the_pages_unsaved() {
    let (desktop, _) = two_pages();
    let path = scratch_dir("missing")
        .join("no such folder")
        .join("scan.pdf");
    let error = block_on(desktop.save(path.clone())).unwrap_err();
    assert!(
        error.starts_with(&format!("Could not save the PDF to {}: ", path.display())),
        "{error}"
    );
    assert!(
        error.ends_with("; choose another location and try again."),
        "{error}"
    );
    assert_eq!(desktop.has_unsaved_pages(), Ok(true));
}

#[test]
fn a_relative_save_path_is_refused() {
    let (desktop, _) = two_pages();
    assert_eq!(
        block_on(desktop.save(PathBuf::from("scan.pdf"))),
        Err("Cannot save to scan.pdf, which is not a full path; choose the file again.".to_owned())
    );
}
