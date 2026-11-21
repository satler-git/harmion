//! Local technical experiment, NOT the Harmion Store implementation.
//! Callback Result's generated JS wrapper throws only after the Rust frame
//! returns. A bare JS shim import keeps Wasm RuntimeError out of Rust recovery.
use js_sys::{Function, Reflect};
use std::{
    cell::Cell,
    sync::atomic::{AtomicU32, Ordering},
};
use wasm_bindgen::{JsCast, prelude::*};

#[wasm_bindgen]
extern "C" {
    type Storage;

    // Comparison only: ordinary Result errors, never a panicking operation.
    #[wasm_bindgen(method, catch, js_name = transactionSync)]
    fn transaction_sync(this: &Storage, callback: &Function) -> Result<JsValue, JsValue>;

    // Legacy one-shot abort comparison, deliberately uncaught.
    #[wasm_bindgen(method, js_name = transactionSync)]
    fn transaction_sync_uncaught(this: &Storage, callback: &Function) -> JsValue;
}

#[wasm_bindgen(module = "bridge-shim")]
extern "C" {
    // MUST NOT use catch: RuntimeError is rethrown by JS, escaping the Rust
    // root export without resuming a potentially inconsistent Rust stack.
    #[wasm_bindgen(js_name = runTransaction)]
    fn shim_transaction(storage: &Storage, callback: &Function) -> JsValue;
}

static IMPORT_RETURNS: AtomicU32 = AtomicU32::new(0);
static TRANSACTION_RETURNS: AtomicU32 = AtomicU32::new(0);

// JS obtains these pointers BEFORE the panic, then reads exported memory
// directly afterward. No post-trap call into Wasm is needed to observe them.
#[wasm_bindgen]
pub fn import_returns_pointer() -> usize {
    IMPORT_RETURNS.as_ptr() as usize
}
#[wasm_bindgen]
pub fn transaction_returns_pointer() -> usize {
    TRANSACTION_RETURNS.as_ptr() as usize
}

#[derive(Clone, Copy)]
enum Boundary {
    CatchComparison,
    Shim,
}
impl Boundary {
    fn name(self) -> &'static str {
        match self {
            Self::CatchComparison => "catch",
            Self::Shim => "shim",
        }
    }
}
#[derive(Debug, PartialEq)]
enum ProbeError {
    Injected(&'static str),
    Host,
    MissingCallback,
    RepeatedCallback,
}

/// Both boundary variants share this generic, non-'static FnOnce wrapper.
/// Original Rust errors stay local; the shim only returns a host-status tag.
fn transaction<T>(
    storage: &Storage,
    boundary: Boundary,
    operation: impl FnOnce() -> Result<T, ProbeError>,
) -> Result<T, ProbeError> {
    let mut operation = Some(operation);
    let mut outcome = None;
    let host_ok = {
        let mut callback = || -> Result<JsValue, JsValue> {
            let Some(operation) = operation.take() else {
                outcome = Some(Err(ProbeError::RepeatedCallback));
                return Err(JsValue::from_str("bridge callback called more than once"));
            };
            outcome = Some(operation());
            match &outcome {
                Some(Ok(_)) => Ok(JsValue::UNDEFINED),
                _ => Err(JsValue::from_str("bridge rollback sentinel")),
            }
        };
        let callback = ScopedClosure::borrow_mut_assert_unwind_safe(&mut callback);
        match boundary {
            Boundary::CatchComparison => storage
                .transaction_sync(callback.as_ref().unchecked_ref())
                .is_ok(),
            Boundary::Shim => {
                let tag = shim_transaction(storage, callback.as_ref().unchecked_ref());
                // First Rust statement after the bare shim import: must NOT run
                // on the panic/trap path. The counter is observable in JS memory.
                IMPORT_RETURNS.fetch_add(1, Ordering::SeqCst);
                Reflect::get(&tag, &JsValue::from_str("ok"))
                    .ok()
                    .and_then(|v| v.as_bool())
                    == Some(true)
            }
        }
        // Scoped callback drops before the borrowed local outcome is read.
    };
    match outcome {
        Some(Err(error)) => Err(error),
        Some(Ok(value)) if host_ok => Ok(value),
        Some(Ok(_)) => Err(ProbeError::Host),
        None => Err(ProbeError::MissingCallback),
    }
}

fn execute(storage: &Storage, query: &str, parameter: Option<&str>) -> Result<(), ProbeError> {
    let sql =
        Reflect::get(storage.as_ref(), &JsValue::from_str("sql")).map_err(|_| ProbeError::Host)?;
    let exec = Reflect::get(&sql, &JsValue::from_str("exec"))
        .map_err(|_| ProbeError::Host)?
        .dyn_into::<Function>()
        .map_err(|_| ProbeError::Host)?;
    match parameter {
        Some(parameter) => exec.call2(&sql, &query.into(), &parameter.into()),
        None => exec.call1(&sql, &query.into()),
    }
    .map(|_| ())
    .map_err(|_| ProbeError::Host)
}

struct Cleanup<'a>(&'a Cell<u32>);
impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

fn run_operation(
    storage: JsValue,
    through: &str,
    mode: &str,
    boundary: Boundary,
) -> Result<String, JsValue> {
    let exact_sequence: u64 = through
        .parse()
        .map_err(|_| JsValue::from_str("invalid u64"))?;
    if exact_sequence.to_string() != through {
        return Err(JsValue::from_str("noncanonical sequence"));
    }
    if !matches!(
        mode,
        "prefix" | "cursor" | "commit" | "fresh" | "host-error" | "panic"
    ) {
        return Err(JsValue::from_str("unknown probe mode"));
    }
    if mode == "panic" && matches!(boundary, Boundary::CatchComparison) {
        return Err(JsValue::from_str(
            "never run a panic through the comparison catch import",
        ));
    }
    let storage: &Storage = storage.unchecked_ref();
    let cleanup = Cell::new(0);
    let mut borrowed_mutations = Vec::new();
    let outcome = transaction(storage, boundary, || {
        let _guard = Cleanup(&cleanup);
        borrowed_mutations.push("entered");
        let id = format!("{}-{mode}", boundary.name());
        execute(
            storage,
            "INSERT INTO bridge_records VALUES (?, 'bytes')",
            Some(&id),
        )?;
        if mode == "prefix" {
            return Err(ProbeError::Injected("after-prefix"));
        }
        execute(
            storage,
            "UPDATE bridge_cursor SET through_value=? WHERE singleton=1",
            Some(through),
        )?;
        if mode == "cursor" {
            return Err(ProbeError::Injected("after-cursor"));
        }
        if mode == "panic" {
            panic!("intentional generic FnOnce shim panic after SQL writes");
        }
        borrowed_mutations.push("completed");
        Ok(73_u32)
    });
    TRANSACTION_RETURNS.fetch_add(1, Ordering::SeqCst);
    let result = match (mode, outcome) {
        ("prefix", Err(ProbeError::Injected("after-prefix"))) => "original-prefix-error",
        ("cursor", Err(ProbeError::Injected("after-cursor"))) => "original-cursor-error",
        ("host-error", Err(ProbeError::Host)) => "host-error-not-success",
        ("commit" | "fresh", Ok(73)) => "committed",
        (_, error) => {
            return Err(JsValue::from_str(&format!(
                "unexpected Rust result: {error:?}"
            )));
        }
    };
    if cleanup.get() != 1
        || borrowed_mutations.len()
            != if matches!(mode, "commit" | "fresh" | "host-error") {
                2
            } else {
                1
            }
    {
        return Err(JsValue::from_str("borrow or ordinary Rust cleanup failed"));
    }
    Ok(format!(
        "{{\"result\":\"{result}\",\"cleanup\":{},\"borrowed_mutations\":{}}}",
        cleanup.get(),
        borrowed_mutations.len()
    ))
}

#[wasm_bindgen]
pub fn run_transaction(storage: JsValue, through: &str, mode: &str) -> Result<String, JsValue> {
    run_operation(storage, through, mode, Boundary::CatchComparison)
}

/// JsValue return avoids generated JS return-string finally/free calls that
/// could otherwise reenter poisoned Wasm while a RuntimeError is propagating.
#[wasm_bindgen]
pub fn run_shim_transaction(
    storage: JsValue,
    through: JsValue,
    mode: JsValue,
) -> Result<JsValue, JsValue> {
    let through = through
        .as_string()
        .ok_or_else(|| JsValue::from_str("sequence must be a string"))?;
    let mode = mode
        .as_string()
        .ok_or_else(|| JsValue::from_str("mode must be a string"))?;
    run_operation(storage, &through, &mode, Boundary::Shim).map(|value| JsValue::from_str(&value))
}

/// Legacy isolated trap comparison. The shim experiment uses the generic FnOnce
/// wrapper above, not this separate callback. Never reuse the trapped instance.
#[wasm_bindgen]
pub fn panic_transaction(storage: JsValue) {
    let storage: &Storage = storage.unchecked_ref();
    let mut callback = || -> JsValue {
        execute(
            storage,
            "INSERT INTO bridge_records VALUES ('legacy-panic', 'discard')",
            None,
        )
        .unwrap();
        execute(
            storage,
            "UPDATE bridge_cursor SET through_value='panic-marker' WHERE singleton=1",
            None,
        )
        .unwrap();
        panic!("intentional one-shot bridge panic after SQL writes");
    };
    let callback = ScopedClosure::borrow_mut_assert_unwind_safe(&mut callback);
    storage.transaction_sync_uncaught(callback.as_ref().unchecked_ref());
}
