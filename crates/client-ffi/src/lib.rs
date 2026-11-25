//! Android SDK native bridge.
//!
//! The Android SDK is Kotlin for UI, lifecycle and storage, but the data plane
//! reuses the existing Rust App API client: pinned TLS, the canonical HRQ/HQU
//! encodings and the subscription cache must match the core exactly, and a
//! second implementation in Kotlin would drift.
//!
//! Every operation is JSON-in/JSON-out over JNI so the ABI stays stable.
pub mod api;
pub mod jni;
pub mod spec;
