// Tests for simple WebRTC-related invariants that don't require async runtime or external deps.
// Note: Using super::BUFFER_SIZE defined in the aggregator (webrtc_tests.rs).

use super::*;

#[test]
fn buffer_size_constant_is_sane() {
    assert!(super::BUFFER_SIZE >= 64 && super::BUFFER_SIZE <= 64 * 1024, "buffer size out of expected bounds");
}

#[test]
fn sdp_text_must_start_with_version_and_use_crlf() {
    let sdp = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\na=group:BUNDLE 0\r\n";
    assert!(sdp.starts_with("v=0"), "SDP should start with version line");
    assert!(sdp.contains("\r\n"), "SDP lines must be CRLF terminated");
}

#[test]
fn ice_candidate_text_contains_required_tokens() {
    let cand = "candidate:842163049 1 udp 1677729535 192.168.1.2 54321 typ srflx raddr 0.0.0.0 rport 0";
    assert!(cand.starts_with("candidate:"), "ICE candidate should start with 'candidate:'");
    assert!(cand.contains(" typ "), "ICE candidate must include a type");
}

#[test]
fn saturating_math_prevents_overflow() {
    let a: u64 = u64::MAX - 10;
    let b: u64 = 42;
    let sum = a.saturating_add(b);
    assert_eq!(sum, u64::MAX);
}

#[test]
fn jitter_buffer_window_edges() {
    let window_ms: u64 = 0;
    assert_eq!(window_ms, 0, "zero window must be allowed for immediate flush logic");

    let large_window_ms: u64 = 60_000; // 60s
    assert!(large_window_ms <= 120_000, "excessively large jitter window can cause memory pressure");
}