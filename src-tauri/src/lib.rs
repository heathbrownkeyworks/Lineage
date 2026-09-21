// Public so the integration tests in tests/ can drive the same code paths
// the commands use.
pub mod assets;
pub mod bsa;
pub mod backup;
pub mod clean;
pub mod detect;
pub mod jslot;
pub mod library;
pub mod nexus;
pub mod ops;
pub mod package;
pub mod rawjson;
pub mod readiness;
pub mod requirements;
pub mod restore;
pub mod scan;
pub mod settings;
pub mod snapshot;

use tauri::Manager;

/// Tint the Windows 11 window border to match the app background so the
/// default light DWM hairline doesn't frame the window (same treatment as
/// Visage — see NOTES.md). No-op if the HWND can't be obtained.
#[cfg(target_os = "windows")]
fn tint_window_border(window: &tauri::WebviewWindow) {
    type RawHwnd = *mut std::ffi::c_void;
    const DWMWA_BORDER_COLOR: u32 = 34;

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: RawHwnd,
            dw_attribute: u32,
            pv_attribute: *const std::ffi::c_void,
            cb_attribute: u32,
        ) -> i32;
    }

    let Ok(hwnd) = window.hwnd() else { return };
    let hwnd_raw: RawHwnd = hwnd.0 as RawHwnd;
    // COLORREF is 0x00BBGGRR. Starfall --sf-void #07070E (the window chrome
    // color behind titlebar and sidebar) → R=0x07 G=0x07 B=0x0E.
    let color: u32 = 0x000E_0707;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd_raw,
            DWMWA_BORDER_COLOR,
            &color as *const u32 as *const std::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        );
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // reqwest is built with rustls-no-provider (aws-lc-rs needs NASM on
    // Windows); install ring as the process-wide TLS crypto provider.
    let _ = rustls::crypto::ring::default_provider().install_default();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            #[cfg(target_os = "windows")]
            {
                if let Some(window) = app.get_webview_window("main") {
                    tint_window_border(&window);
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = &app;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            settings::get_settings,
            settings::save_settings,
            settings::default_backup_dir,
            settings::validate_backup_dir,
            detect::detect_environment,
            detect::list_mo2_profiles,
            detect::default_jslot_roots,
            scan::scan_jslots,
            backup::get_backup_status,
            backup::run_backup,
            restore::list_backups,
            restore::inspect_backup,
            restore::restore_backup,
            snapshot::list_snapshots,
            snapshot::restore_snapshot,
            snapshot::snapshot_stats,
            snapshot::clear_snapshots,
            ops::inspect_preset,
            ops::clean_preset,
            ops::batch_scan,
            ops::batch_clean,
            nexus::validate_nexus_key,
            nexus::get_rate_limit,
            assets::find_assets,
            assets::review_collection,
            assets::review_refresh,
            requirements::list_presets_in,
            requirements::requirements_for,
            requirements::render_requirements,
            package::pack_release,
            readiness::readiness_for,
            readiness::readiness_sweep,
            library::library_list,
            library::library_save_entries,
            library::library_delete_entry,
            library::library_restore_seed,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
