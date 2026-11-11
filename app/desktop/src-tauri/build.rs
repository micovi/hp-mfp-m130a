//! Generates the Tauri context, and permissions for exactly the app's own commands.

use tauri_build::{AppManifest, Attributes};

/// The commands in `src/commands.rs`; each gets an `allow-<name>` permission for capabilities.
const COMMANDS: &[&str] = &[
    "printer_connected",
    "scan_page",
    "cancel_scan",
    "delete_page",
    "reorder_pages",
    "save_pdf",
    "has_unsaved_pages",
    "quit",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = AppManifest::new().commands(COMMANDS);
    tauri_build::try_build(Attributes::new().app_manifest(manifest))?;
    Ok(())
}
