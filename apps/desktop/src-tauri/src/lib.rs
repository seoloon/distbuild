pub mod commands;
pub mod peers;
pub mod state;
pub mod tls_verify;
pub mod ws_client;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tls_verify::install_crypto_provider();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(state::build_app_state())
        .invoke_handler(tauri::generate_handler![
            commands::discover_workers,
            commands::pair_worker,
            commands::confirm_pair,
            commands::submit_job,
            commands::cancel_job,
            commands::open_artifacts_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
