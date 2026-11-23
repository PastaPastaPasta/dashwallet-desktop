#![no_main]

// `decode` catches rqrr's panics (#209), but libFuzzer's hook aborts on any
// panic: here a panic is only printed, and one `decode` lets out still
// aborts in libfuzzer-sys's wrapper.
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let abort = std::panic::take_hook();
    std::panic::set_hook(Box::new(|info| eprintln!("{info}")));
    dw_fuzz::qr_image(data);
    std::panic::set_hook(abort);
});
