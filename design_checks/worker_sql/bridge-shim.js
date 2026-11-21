// Local experiment only. All generated Rust exports share this poison state.
let poisoned = false;
let blockedEntries = 0;

export function poisonInstance() { poisoned = true; }
export function poisonState() { return { poisoned, blocked_entries: blockedEntries }; }

export function guardedRustEntry(operation) {
  if (poisoned) {
    blockedEntries += 1;
    throw new Error("Wasm instance poisoned; fresh process required");
  }
  try {
    return operation();
  } catch (error) {
    if (error instanceof WebAssembly.RuntimeError) poisonInstance();
    throw error;
  }
}

// Rust imports this WITHOUT #[wasm_bindgen(catch)]. Ordinary host/rollback
// errors return a tag; an abort/trap must escape all Rust frames immediately.
export function runTransaction(storage, callback) {
  if (poisoned) throw new WebAssembly.RuntimeError("attempted poisoned transaction");
  try {
    storage.transactionSync(callback);
    return { ok: true };
  } catch (error) {
    if (error instanceof WebAssembly.RuntimeError) {
      poisonInstance();
      throw error;
    }
    return { ok: false, code: "host-error" };
  }
}
