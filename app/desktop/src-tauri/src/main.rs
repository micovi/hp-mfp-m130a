//! M130a Scan: scans multi-page documents from the HP M130a into one PDF.

fn main() -> Result<(), tauri::Error> {
    scan_desktop::run()
}
