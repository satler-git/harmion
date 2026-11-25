//! Android node FFI surface.
//!
//! The JNI layer is deliberately thin: every operation is a JSON request and a
//! JSON response handled by [`api`], so the protocol is testable on the host and
//! the Kotlin SDK never links the Rust ABI directly.
//!
//! Handles are process-local and require an explicit `stop`; the Android service
//! owns exactly one runtime per node directory.
pub mod api;
pub mod jni;
