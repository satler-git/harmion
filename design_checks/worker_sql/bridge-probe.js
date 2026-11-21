// Actual Rust/Wasm callbacks against actual DO SQL, local technical test ONLY.
import * as rustExports from "./bridge.js";
import wasmModule from "./bridge_bg.wasm";
import { guardedRustEntry, poisonState } from "bridge-shim";

// Every generated function, including init/pointer getters/future exports, goes
// through one boundary. No untrusted request chooses a function or arguments.
const callRust = (name, ...args) => guardedRustEntry(() => rustExports[name](...args));
const wasm = callRust("initSync", { module: wasmModule });
const importPointer = callRust("import_returns_pointer");
const transactionPointer = callRust("transaction_returns_pointer");
// Safe to use AFTER a trap: plain JS reads of Wasm memory, no exported calls.
const counters = () => {
  const view = new DataView(wasm.memory.buffer);
  return { import_returns: view.getUint32(importPointer, true), transaction_returns: view.getUint32(transactionPointer, true) };
};
const check = (condition, message) => { if (!condition) throw new Error(message); };
const equal = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const exactU64 = "18446744073709551615";

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.hostname !== "127.0.0.1" || request.headers.has("Origin") ||
        request.method !== "GET" || url.search ||
        !["/state", "/run", "/panic", "/shim-run", "/shim-panic", "/fresh"].includes(url.pathname)) {
      return new Response("Local test routes only", { status: 403 });
    }
    return env.PROBE.get(env.PROBE.idFromName("one-local-bridge-object")).fetch(request);
  }
};

export class BridgeProbe {
  constructor(state) {
    this.storage = state.storage;
    this.sql = state.storage.sql;
    this.storage.transactionSync(() => {
      this.sql.exec("CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)");
      this.sql.exec("INSERT OR IGNORE INTO metadata VALUES ('node_id', ?)", crypto.randomUUID());
      this.sql.exec("INSERT OR IGNORE INTO metadata VALUES ('activations', '0')");
      this.sql.exec("UPDATE metadata SET value=CAST(CAST(value AS INTEGER)+1 AS TEXT) WHERE key='activations'");
      this.sql.exec("CREATE TABLE IF NOT EXISTS bridge_records (id TEXT PRIMARY KEY, bytes TEXT NOT NULL)");
      this.sql.exec("CREATE TABLE IF NOT EXISTS bridge_cursor (singleton INTEGER PRIMARY KEY, through_value TEXT NOT NULL)");
      this.sql.exec("INSERT OR IGNORE INTO bridge_cursor VALUES (1, '0')");
    });
  }
  state() {
    return {
      node_id: this.sql.exec("SELECT value FROM metadata WHERE key='node_id'").one().value,
      activations: this.sql.exec("SELECT value FROM metadata WHERE key='activations'").one().value,
      records: this.sql.exec("SELECT id, bytes FROM bridge_records ORDER BY id").toArray(),
      cursor: this.sql.exec("SELECT through_value FROM bridge_cursor").one().through_value,
    };
  }
  rust(through, mode, shim = false, storage = this.storage) {
    return JSON.parse(callRust(shim ? "run_shim_transaction" : "run_transaction", storage, through, mode));
  }
  run(shim = false) {
    const baseline = this.state();
    check(baseline.records.length === (shim ? 1 : 0), "run requires expected fresh comparison state");
    const passed = [];
    for (const [mode, expected] of [["prefix", "original-prefix-error"], ["cursor", "original-cursor-error"]]) {
      const result = this.rust(exactU64, mode, shim);
      check(result.result === expected && result.cleanup === 1 && result.borrowed_mutations === 1,
        "original Rust error/cleanup/borrowed state lost");
      check(equal(this.state(), baseline), "Rust Err SQL rollback failed");
      passed.push(`${shim ? "shim" : "catch"}-rust-${mode}-original-error-SQL-rollback-cleanup-borrow`);
    }
    if (shim) {
      // Throw AFTER Rust callback returns success, but INSIDE the REAL SQL
      // transaction. This tests rollback and host-error conversion, not an error
      // fabricated after commit. No arbitrary request controls this facade.
      const actual = this.storage;
      const facade = {
        sql: actual.sql,
        transactionSync(callback) {
          return actual.transactionSync(() => {
            callback();
            throw new Error("injected host error after successful Rust callback");
          });
        },
      };
      const result = this.rust(exactU64, "host-error", true, facade);
      check(result.result === "host-error-not-success" && result.cleanup === 1 && result.borrowed_mutations === 2,
        "host failure incorrectly returned success or lost cleanup");
      check(equal(this.state(), baseline), "host failure after callback success failed SQL rollback");
      passed.push("shim-host-error-after-callback-success-SQL-rollback-not-false-success");
    }
    const success = this.rust(exactU64, "commit", shim);
    check(success.result === "committed" && success.cleanup === 1 && success.borrowed_mutations === 2,
      "Rust success/cleanup/borrowed state failed");
    check(this.state().records.length === baseline.records.length + 1 && this.state().cursor === exactU64,
      "Rust commit or u64 text binding failed");
    passed.push(`${shim ? "shim" : "catch"}-rust-success-commit-cleanup-u64-max-exact`);
    check(!poisonState().poisoned, "normal Rust errors incorrectly poisoned instance");
    return { passed, state: this.state() };
  }
  panic(shim = false) {
    const before = this.state();
    check(before.records.length === (shim ? 2 : 1) && before.cursor === exactU64, "normal tests before panic required");
    const beforeCounters = counters();
    let trapped = false;
    try {
      if (shim) callRust("run_shim_transaction", this.storage, "18446744073709551614", "panic");
      else callRust("panic_transaction", this.storage);
    } catch (error) { trapped = error instanceof WebAssembly.RuntimeError; }
    check(trapped && poisonState().poisoned, "expected trap and global instance poison");
    const afterCounters = counters();
    check(equal(beforeCounters, afterCounters), "Rust resumed past import or generic transaction after trap");
    check(equal(before, this.state()), "Wasm trap did not roll back SQL prefix/cursor");
    // Test every generated function, including pointer getters and init, without
    // invoking any: the shared JS guard must reject BEFORE touching Wasm.
    const names = Object.keys(rustExports).filter(name => typeof rustExports[name] === "function");
    for (const name of names) {
      let blocked = false;
      try { callRust(name); }
      catch (error) { blocked = error.message === "Wasm instance poisoned; fresh process required"; }
      check(blocked, `unguarded generated export: ${name}`);
    }
    check(poisonState().blocked_entries === names.length && equal(beforeCounters, counters()),
      "entry guard did not reject every export without Rust continuation");
    return {
      passed: [
        `${shim ? "same-generic-FnOnce-shim" : "legacy-one-shot"}-panic-SQL-rollback`,
        "panic-no-Rust-import-or-transaction-continuation",
        `poison-all-${names.length}-generated-export-functions-blocked-before-Wasm`,
      ],
      diagnostics: { before_counters: beforeCounters, after_counters: afterCounters, poison: poisonState() },
      state: this.state(),
    };
  }
  fresh() {
    check(this.state().activations !== "1" && !poisonState().poisoned, "fresh unpoisoned process required");
    check(counters().import_returns === 0 && counters().transaction_returns === 0, "Wasm counters not reset by process restart");
    const before = this.state().records.length;
    const result = this.rust(exactU64, "fresh", true);
    check(result.result === "committed" && this.state().records.length === before + 1,
      "fresh Wasm instance did not commit after restart");
    check(counters().import_returns === 1 && counters().transaction_returns === 1, "fresh Rust continuation not observed");
    return { passed: ["fresh-process-unpoisoned-Wasm-counters-reset-and-commit"], state: this.state() };
  }
  async fetch(request) {
    try {
      const path = new URL(request.url).pathname;
      const result = path === "/run" ? this.run() : path === "/shim-run" ? this.run(true) :
        path === "/panic" ? this.panic() : path === "/shim-panic" ? this.panic(true) :
        path === "/fresh" ? this.fresh() : this.state();
      await this.storage.sync();
      return Response.json(result);
    } catch (error) {
      return Response.json({ error: error.message }, { status: 500 });
    }
  }
}
