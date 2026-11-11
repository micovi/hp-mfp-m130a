//! The M130a Scan desktop app: a Tauri window over `scan-core`.
//!
//! The UI talks to [`commands`], shows pages through the `page://localhost/<id>` URI scheme, and
//! is asked to confirm (event `confirm-close`) before a window with unsaved pages closes.

pub mod commands;
pub mod pages;

use tauri::http::{HeaderValue, Method, Request, Response, StatusCode, header};
use tauri::menu::{Menu, MenuBuilder, MenuEvent, MenuItem, SubmenuBuilder, WINDOW_SUBMENU_ID};
use tauri::{AppHandle, Emitter, Manager, Runtime, Window, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::commands::{Desktop, STOP_WAIT, quit_app};
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
    match desktop.page(id) {
        Ok(Some(page)) => {
            let mut response = Response::new(page.jpeg().to_vec());
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

/// Keeps a window with unsaved pages open and asks whether to quit; a page list broken by an
/// internal error counts as unsaved. A window that closes stops the running scan first.
fn guard_close<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    let desktop = window.state::<Desktop>();
    if desktop.has_unsaved_pages().unwrap_or(true) {
        api.prevent_close();
        ask_to_quit(window.app_handle());
    } else {
        desktop.stop_scan(STOP_WAIT);
    }
}

/// Asks the UI whether to quit (it answers by calling `quit`); when the UI cannot be reached,
/// asks with a native dialog instead.
fn ask_to_quit<R: Runtime>(app: &AppHandle<R>) {
    if app.emit(CONFIRM_CLOSE, ()).is_ok() {
        return;
    }
    let handle = app.clone();
    app.dialog()
        .message("You have unsaved pages. Quit anyway?")
        .title("M130a Scan")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Quit".to_owned(),
            "Cancel".to_owned(),
        ))
        .show(move |quit| {
            if quit {
                quit_app(&handle, &handle.state::<Desktop>());
            }
        });
}

/// Quit (⌘Q) closes the window, so it passes through [`guard_close`]. Without a window to close,
/// it asks directly when there are unsaved pages and quits otherwise.
fn close_or_ask<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW)
        && window.close().is_ok()
    {
        return;
    }
    let desktop = app.state::<Desktop>();
    if desktop.has_unsaved_pages().unwrap_or(true) {
        ask_to_quit(app);
    } else {
        quit_app(app, &desktop);
    }
}

/// The macOS menu bar of Tauri's default menu, except that Quit is the app's own item: the
/// predefined one ends the app at once, skipping the unsaved-pages question.
fn menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let name = app.package_info().name.clone();
    let quit = MenuItem::with_id(
        app,
        QUIT_MENU_ID,
        format!("Quit {name}"),
        true,
        Some("CmdOrCtrl+Q"),
    )?;
    let app_menu = SubmenuBuilder::new(app, &name)
        .about(None)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .item(&quit)
        .build()?;
    let file = SubmenuBuilder::new(app, "File").close_window().build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let view = SubmenuBuilder::new(app, "View").fullscreen().build()?;
    let window = SubmenuBuilder::with_id(app, WINDOW_SUBMENU_ID, "Window")
        .minimize()
        .maximize()
        .separator()
        .close_window()
        .build()?;
    MenuBuilder::new(app)
        .items(&[&app_menu, &file, &edit, &view, &window])
        .build()
}

fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: &MenuEvent) {
    if event.id() == QUIT_MENU_ID {
        close_or_ask(app);
    }
}

#[cfg(test)]
mod tests;
