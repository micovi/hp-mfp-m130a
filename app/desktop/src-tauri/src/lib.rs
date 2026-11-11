//! The M130a Scan desktop app: a Tauri window over `scan-core`.
//!
//! The UI talks to [`commands`], shows pages through the `page://localhost/<id>` URI scheme, and
//! is asked to confirm (event `confirm-close`) before a window with unsaved pages closes.

pub mod commands;
pub mod pages;

use tauri::http::{HeaderValue, Method, Request, Response, StatusCode, header};
use tauri::menu::{Menu, MenuEvent, MenuItem};
use tauri::{AppHandle, Emitter, Manager, Runtime, Window, WindowEvent};

use crate::commands::Desktop;
use crate::pages::PageId;

/// The event the UI answers by asking whether to quit, then calling `quit`.
pub const CONFIRM_CLOSE: &str = "confirm-close";

const MAIN_WINDOW: &str = "main";
const QUIT_MENU_ID: &str = "quit-guarded";

/// Runs the app until it quits.
///
/// # Errors
///
/// Tauri's error when the app cannot start.
pub fn run() -> Result<(), tauri::Error> {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Desktop::new())
        .menu(menu)
        .on_menu_event(|app, event| on_menu_event(app, &event))
        .on_window_event(guard_close)
        .register_uri_scheme_protocol("page", |context, request| {
            page_response(context.app_handle().state::<Desktop>().inner(), &request)
        })
        .invoke_handler(tauri::generate_handler![
            commands::printer_connected,
            commands::scan_page,
            commands::cancel_scan,
            commands::delete_page,
            commands::reorder_pages,
            commands::save_pdf,
            commands::has_unsaved_pages,
            commands::quit,
        ])
        .run(tauri::generate_context!())
}

/// Serves `page://localhost/<id>`: the page's JPEG, 404 for an unknown page, 405 for anything
/// but GET.
pub fn page_response(desktop: &Desktop, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    if request.method() != Method::GET {
        return text_response(StatusCode::METHOD_NOT_ALLOWED, "Only GET is supported.");
    }
    let Some(id) = PageId::from_path(request.uri().path()) else {
        return text_response(StatusCode::NOT_FOUND, "No such page.");
    };
    match desktop.page_jpeg(id) {
        Ok(Some(jpeg)) => {
            let mut response = Response::new(jpeg);
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
            response
        }
        Ok(None) => text_response(StatusCode::NOT_FOUND, "No such page."),
        Err(message) => text_response(StatusCode::INTERNAL_SERVER_ERROR, &message),
    }
}

fn text_response(status: StatusCode, text: &str) -> Response<Vec<u8>> {
    let mut response = Response::new(text.as_bytes().to_vec());
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

/// Keeps a window with unsaved pages open and asks the UI to confirm; a page list broken by an
/// internal error counts as unsaved. When the UI cannot be asked, the window closes.
fn guard_close<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    let unsaved = window
        .state::<Desktop>()
        .has_unsaved_pages()
        .unwrap_or(true);
    if unsaved && window.emit(CONFIRM_CLOSE, ()).is_ok() {
        api.prevent_close();
    }
}

/// Tauri's default menu, with Quit (⌘Q) closing the window through [`guard_close`] instead of
/// ending the app at once, which would skip the unsaved-pages question.
fn menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::default(app)?;
    let Some(app_menu) = menu
        .items()?
        .into_iter()
        .next()
        .and_then(|item| item.as_submenu().cloned())
    else {
        return Ok(menu);
    };
    let items = app_menu.items()?;
    let quit_at = items.iter().rposition(|item| {
        item.as_predefined_menuitem()
            .and_then(|predefined| predefined.text().ok())
            .is_some_and(|text| text.starts_with("Quit"))
    });
    if let Some(position) = quit_at {
        let text = format!("Quit {}", app.package_info().name);
        let quit = MenuItem::with_id(app, QUIT_MENU_ID, text, true, Some("CmdOrCtrl+Q"))?;
        app_menu.remove_at(position)?;
        app_menu.insert(&quit, position)?;
    }
    Ok(menu)
}

fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: &MenuEvent) {
    if event.id() != QUIT_MENU_ID {
        return;
    }
    match app.get_webview_window(MAIN_WINDOW) {
        Some(window) => {
            if window.close().is_err() {
                app.exit(0);
            }
        }
        None => app.exit(0),
    }
}

#[cfg(test)]
mod tests;
