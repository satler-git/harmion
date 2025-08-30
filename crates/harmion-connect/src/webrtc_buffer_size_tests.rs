//! Unit-style tests colocated near the webrtc module for BUFFER_SIZE stability.
//! Testing framework: Rust built-in test harness (libtest).

#[cfg(test)]
mod tests {
    // Re-export path to avoid relative module coupling.
    use crate::webrtc;

    #[test]
    fn buffer_size_matches_contract() {
        assert_eq!(webrtc::BUFFER_SIZE, 256);
        assert!(webrtc::BUFFER_SIZE.is_power_of_two(), "Prefer power-of-two buffer sizes for alignment and performance");
        assert!(webrtc::BUFFER_SIZE >= 64, "Sanity bound: too small for practical RTC framing");
    }

    #[test]
    fn buffer_size_nonzero() {
        assert_ne!(webrtc::BUFFER_SIZE, 0, "BUFFER_SIZE must never be zero");
    }
}