/// Commands the renderer may call. Each gets an `allow-<name>` permission that
/// `capabilities/main.json` grants; `probe_unlisted` gets one too but is never
/// granted (the U6 refusal probe).
const COMMANDS: &[&str] = &[
    "launch_info",
    "open_network",
    "list_wallets",
    "lock_status",
    "unlock",
    "lock",
    "balance",
    "sync_status",
    "history_page",
    "tx_detail",
    "startup_mark",
    "selftest_report",
    "selftest_checkpoint",
    "ipc_bench_rows",
    "history_page_timed",
    "probe_unlisted",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("tauri build script");
}
