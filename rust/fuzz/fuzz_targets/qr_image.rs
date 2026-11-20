#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| dw_fuzz::qr_image(data));
