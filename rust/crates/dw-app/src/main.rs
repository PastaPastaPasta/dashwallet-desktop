// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // SAFETY: first in `main`, while the process has one thread.
    #[cfg(target_os = "linux")]
    unsafe {
        dw_app::webkit_renderer_without_gpu()
    };
    dw_app::run();
}
