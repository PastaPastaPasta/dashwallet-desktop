// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // First, while the process has one thread: it may set an environment variable.
    #[cfg(target_os = "linux")]
    dw_app::webkit_renderer_without_gpu();
    dw_app::run();
}
