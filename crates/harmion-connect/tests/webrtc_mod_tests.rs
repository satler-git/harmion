// Integration tests for harmion-connect webrtc surface.
// Testing framework: Rust built-in test harness (libtest). No external test deps.

/// BEGIN: webrtc.rs basic module and const tests (auto-generated)
/// These tests focus on the recent diff affecting webrtc.rs: module declarations and BUFFER_SIZE.

// Try to import the crate under its package name. If the crate is not named "harmion_connect"
// in Cargo.toml, update the path below accordingly. We use `extern crate` only if necessary.
#[allow(unused_imports)]
use harmion_connect as _crate_name_probe;

#[test]
fn webrtc_buffer_size_is_expected() {
    const EXPECTED: usize = 256;
    assert_eq!(harmion_connect::webrtc::BUFFER_SIZE, EXPECTED, "webrtc::BUFFER_SIZE changed unexpectedly");
}

#[test]
fn webrtc_modules_compile_and_are_linked() {
    let _size = harmion_connect::webrtc::BUFFER_SIZE;
    assert!(_size > 0, "buffer size should be positive");
}
/// END: webrtc.rs basic module and const tests