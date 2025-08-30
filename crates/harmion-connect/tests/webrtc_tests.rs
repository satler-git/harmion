// TESTING FRAMEWORK NOTE: Using Rust's built-in test harness. Async tests inside the crate use #[tokio::test],
// but integration tests here remain synchronous and avoid new dev-dependencies.

#![allow(dead_code)] // TODO: remove this
mod pool;
mod signal;
mod simple;

const BUFFER_SIZE: usize = 256;

#[test]
fn buffer_size_is_positive_and_reasonable() {
    assert!(BUFFER_SIZE > 0, "BUFFER_SIZE should be > 0");
    assert!(BUFFER_SIZE <= 1 << 20, "BUFFER_SIZE should not exceed 1 MiB");
}

#[test]
fn aggregator_modules_link() {
    // Compiles if submodules are wired correctly via this aggregator.
    let _ = BUFFER_SIZE;
}