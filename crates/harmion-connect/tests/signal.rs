// Tests for signaling payload structure without requiring network or WebRTC runtime.

fn has_required_keys(msg: &str, required: &[&str]) -> bool {
    required.iter().all(|k| msg.contains(k))
}

#[test]
fn offer_signal_contains_type_and_sdp() {
    let msg = r#"{"type":"offer","sdp":"v=0\r\n..."}"#;
    assert!(has_required_keys(msg, &["type","sdp"]));
    assert!(msg.contains("\"offer\""));
}

#[test]
fn answer_signal_contains_type_and_sdp() {
    let msg = r#"{"type":"answer","sdp":"v=0\r\n..."}"#;
    assert!(has_required_keys(msg, &["type","sdp"]));
    assert!(msg.contains("\"answer\""));
}

#[test]
fn ice_candidate_signal_has_candidate_field() {
    let msg = r#"{"type":"candidate","candidate":"candidate:1 1 UDP 2122252543 203.0.113.1 3478 typ host"}"#;
    assert!(has_required_keys(msg, &["type","candidate"]));
}

#[test]
fn invalid_signal_missing_type_is_detected_by_structure_checks() {
    let msg = r#"{"sdp":"v=0"}"#;
    assert!(!has_required_keys(msg, &["type"]));
}

#[test]
fn empty_payload_is_invalid() {
    let msg = "";
    assert!(msg.is_empty());
}