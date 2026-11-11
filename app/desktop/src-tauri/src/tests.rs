use scan_core::testing::fixture;
use scan_core::{ColorMode, Page, ScanSettings};
use tauri::async_runtime::block_on;
use tauri::http::{Method, Request, StatusCode, header};

use super::*;

fn get(uri: &str) -> Request<Vec<u8>> {
    Request::builder().uri(uri).body(Vec::new()).unwrap()
}

fn desktop_with_a_page() -> (Desktop, PageId, Vec<u8>) {
    let desktop = Desktop::new();
    let jpeg = fixture("synthetic-grey-75dpi.jpg").unwrap();
    let page = Page::new(jpeg.clone(), 75).unwrap();
    let settings = ScanSettings {
        color: ColorMode::Grey,
        dpi: 75,
    };
    let id = block_on(desktop.scan(settings, move |_, _| Ok(page)))
        .unwrap()
        .id;
    (desktop, id, jpeg)
}

#[test]
fn a_page_is_served_as_its_jpeg() {
    let (desktop, id, jpeg) = desktop_with_a_page();
    let response = page_response(&desktop, &get(&format!("page://localhost/{id}")));
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(*response.body(), jpeg);
}

#[test]
fn unknown_or_malformed_page_ids_are_not_found() {
    let (desktop, id, _) = desktop_with_a_page();
    desktop.delete(id).unwrap();
    for uri in [
        format!("page://localhost/{id}"),
        "page://localhost/".to_owned(),
        "page://localhost/abc".to_owned(),
        "page://localhost/1/2".to_owned(),
    ] {
        let response = page_response(&desktop, &get(&uri));
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(response.body().as_slice(), b"No such page.", "{uri}");
    }
}

#[test]
fn only_get_is_served() {
    let (desktop, id, _) = desktop_with_a_page();
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!("page://localhost/{id}"))
        .body(Vec::new())
        .unwrap();
    assert_eq!(
        page_response(&desktop, &request).status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
}
