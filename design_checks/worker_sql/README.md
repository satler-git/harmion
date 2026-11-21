# Local workerd SQL Durable Object spike

This contains **local JavaScript runtime and isolated Rust/Wasm bridge experiments**,
not the Harmion Worker adapter, peer service, bootstrap mechanism, authentication
policy, or a deployable service. All artifacts and dependencies are scoped to this
directory. The standalone Rust workspace is not part of Harmion's root workspace;
no production dependencies or Worker crate are introduced.

## Run locally

From the repository root, with network access for the initial npm download:

```sh
nix shell nixpkgs#nodejs nixpkgs#python3 -c npm ci --prefix design_checks/worker_sql --ignore-scripts --no-audit --no-fund
nix shell nixpkgs#nodejs nixpkgs#python3 -c python3 design_checks/worker_sql/check.py
```

The exact dependency is `workerd@1.20261120.1`; [package-lock.json](package-lock.json)
records package integrity. The driver uses the platform's native executable in the
optional npm binary package directly, so workerd's npm postinstall script and Node
launcher are unnecessary. Do not omit optional dependencies. Tested on Linux x86-64;
the driver also locates the Linux arm64 package when present, but that platform has
not been run here. Other platforms can supply `--workerd /absolute/native/binary`;
this is not a claim that those platforms or arbitrary versions were verified.
A runtime loader/library failure should be reported, not treated as a passing test.
The nixpkgs input is the caller's configured input; this directory does not pin a
complete Nix build environment.

When running through an agent harness, launch the driver as a managed background
job, retain its job ID, collect its output, and cancel that job if abandoning it.
The driver supervises every workerd child and stops/waits for it on success or error.
There is no daemon left running after completion. A bounded local HTTP readiness
retry is internal to the driver, not a command to alter any system service.

### Isolation

[check.py](check.py) binds an IPv4 socket to **127.0.0.1:0**, then passes that already
bound socket to workerd via `--socket-fd`. It never binds a wildcard address or uses
a fixed application/GUI port. Persistence lives in a fresh owner-only temporary
directory, reused only across this test's process restarts and removed afterward.
No existing node/DB is opened. The Cap'n Proto [configuration](config.capnp) has no
socket address or storage path of its own: these must be supplied by the driver.
It explicitly overrides the implicit internet service with an empty allowlist.

Only the fixed `/state` and `/run` GET test routes are accepted. Unknown routes,
query strings, Origin headers, a non-loopback Host, and non-GET methods are rejected.
These guards are **not authentication**; local processes can invoke the probe while
it runs. There is no Harmion data or credential. Never expose it via a proxy, public
socket, tunnel, Wrangler deployment, or Cloudflare account. No Wrangler, account,
API token, service installation, CA change, or host network configuration is used.

## What is tested

[probe.js](probe.js) uses the actual `state.storage.sql` and
`state.storage.transactionSync` APIs of a SQLite-enabled Durable Object:

1. Throw after inserting a normal multi-record prefix: all records roll back and
   the cursor remains unchanged.
2. Throw after updating both records and cursor: both roll back.
3. A successful page commits; repeating an ID with identical BLOB bytes creates
   no extra record while the cursor can advance.
4. A mismatched same ID throws after an otherwise valid prefix; prefix and cursor
   roll back. These are **probe-only** ID checks, not Harmion's validator/codec.
5. Returning an `Error` object as an ordinary callback value **commits** writes.
   Throwing versus returning an error is a critical requirement for a future
   Rust/Wasm bridge; a Rust `Result::Err` returned as data must not become success.
6. The values `9007199254740993`, `-9007199254740993`, `9223372036854775807`, and
   `-9223372036854775808` survive conversion of decimal **string** parameters to
   SQLite INTEGER with `CAST(? AS INTEGER)`, followed by `CAST(integer_value AS TEXT)`.
   Parallel TEXT values and BLOBs of the original UTF-8 decimal bytes also survive
   exactly. BLOBs are byte carriers here, not a proposed production numeric schema.
7. Reading the first value as a JS Number instead observes `9007199254740992`.
   The unsafe Number path is reported explicitly and is not used to verify exactness.
8. A persisted random node-like ID, activation counter, successful page/cursor,
   integer/text/BLOB rows survive a graceful TERM restart and then a SIGKILL/restart
   of the actual workerd process. Each activation increments the persisted counter.
   The kill occurs **after** a response and explicit `storage.sync()`, not during a
   transaction; this does not test crash-during-commit or power failure.

SQL statements and callback execution are synchronous; no network await occurs in
transaction callbacks. An explicit storage flush is awaited before returning test
outcomes. Nothing here establishes Cloudflare multi-region durability.

## Observed result

On the development Linux x86-64 environment, the pinned binary reported
`workerd 2026-11-20`. The complete probe passed, including both thrown rollback
boundaries, duplicate/mismatch cases, the ordinary-returned-error commit hazard,
exact 64-bit string/BLOB cases, the Number rounding observation, and both process
restart cases. See the driver's live PASS output for a fresh run; generated data
and execution logs are not committed.

The JavaScript-only run supports using a synchronous SQL transaction boundary in a
future adapter, but does **not** prove the Rust-specific properties. The separate
Rust bridge experiment below adds narrowly scoped evidence, not full coverage.
Neither experiment proves:

- workers-rs integration, general panic/unwind recovery, cross-target wasm
  portability, or full Store trait coverage;
- Harmion record semantics, domain validation, signatures, membership state,
  authentication, bootstrap, authorization, management API, or numeric query order;
- failure partway through disk commit, disk-full behavior, concurrent page sessions,
  long-term schema migrations, actual Cloudflare deployment behavior, limits or cost.

Do not weaken the existing Store contract or choose the remote security policy
based on these probes. A complete production adapter remains unimplemented.

## Actual Rust/Wasm callback bridge

The isolated [Rust workspace](rust_bridge/Cargo.toml) and its
[lockfile](rust_bridge/Cargo.lock) pin `wasm-bindgen=0.2.130` and
`js-sys=0.3.107`. It has real exported callback code, no Harmion dependency and no
root-workspace membership. `pkg/` and `target/` are generated and ignored.
The CLI generator must match the pinned wasm-bindgen version. The development
Nix input provides `wasm-bindgen-cli 0.2.130`; verify this before generation.
The Rust toolchain must already have `wasm32-unknown-unknown` std available.

From the repository root, after the npm install above:

```sh
cargo build --manifest-path design_checks/worker_sql/rust_bridge/Cargo.toml --target wasm32-unknown-unknown --release --locked
nix shell nixpkgs#wasm-bindgen-cli -c wasm-bindgen --version
# The preceding command must report 0.2.130.
nix shell nixpkgs#wasm-bindgen-cli -c wasm-bindgen --target web --out-dir design_checks/worker_sql/rust_bridge/pkg --out-name bridge design_checks/worker_sql/rust_bridge/target/wasm32-unknown-unknown/release/harmion_worker_sql_bridge_probe.wasm
nix shell nixpkgs#nodejs nixpkgs#python3 -c python3 design_checks/worker_sql/bridge_check.py

cargo fmt --manifest-path design_checks/worker_sql/rust_bridge/Cargo.toml -- --check
cargo clippy --manifest-path design_checks/worker_sql/rust_bridge/Cargo.toml --target wasm32-unknown-unknown --release --locked -- -D warnings
```

The first Cargo build may download the locked dependencies. The lockfile covers
Rust packages, not the complete compiler/Nix closure. Tested compiler:
`rustc 1.99.1 (58cf8f9c3 2026-10-13)`. Cargo and CLI must be available separately;
no tool installation or root workspace build is performed by the driver.

[bridge-config.capnp](bridge-config.capnp) embeds the generated JS and Wasm as
separate named modules. [bridge-probe.js](bridge-probe.js) imports the compiled
Wasm module explicitly and calls `initSync({ module: wasmModule })` using
`--target web` output. No asynchronous fetch-based Wasm compilation or generated
file patching is required. [bridge_check.py](bridge_check.py) reuses the original
loopback prebound-socket/temp-disk process supervisor. It has `/state`, `/run`,
`/shim-run`, `/shim-panic`, `/panic` (legacy comparison), and `/fresh` GET-only
routes and the same non-auth local guards; never expose this test configuration.

### Error/borrow/destructor boundary

[The Rust implementation](rust_bridge/src/lib.rs) provides **one generic `FnOnce`
transaction wrapper** with two host-boundary variants. Both accept non-`'static`
references. A `ScopedClosure::borrow_mut_assert_unwind_safe` callback borrows its
local operation and result slots; it does not leak a closure, demand `'static`, or
use `unsafe`. The operation captures a mutable Vec and cleanup counter by reference.

The initial `CatchComparison` boundary keeps the direct
`#[wasm_bindgen(method, catch)]` import, **for normal Result errors only**. A panic
must not be recovered by that import: resuming Rust after an aborted callback could
continue with inconsistent local/borrow state. The new `Shim` boundary imports
[bridge-shim.js](bridge-shim.js) **without `catch`** and uses this flow:

1. A normal operation error stays in Rust's local result slot with its original
   enum/variant/data. The callback's Rust `Drop` guard runs while returning
   `Result<JsValue, JsValue>::Err`.
2. The generated **JavaScript callback wrapper** throws only **after the Rust
   callback frame returns**. Actual DO `transactionSync` rolls back SQL.
3. The shim catches ordinary host errors and returns a tagged `{ ok: false,
   code: "host-error" }` value. The scoped callback drops normally, and the generic
   wrapper returns the original Rust error (or Host error if the operation had
   succeeded). It never calls `throw_str` inside the Rust callback.
4. A `WebAssembly.RuntimeError` is different: the shim poisons the entire Rust
   instance and **rethrows**. Its bare import cannot catch the trap back into Rust.

Actual runtime tests passed normal prefix/cursor errors in both boundary variants:
SQL rollback, distinct original Rust error, one cleanup, expected borrowed Vec
mutation count, then a successful commit without poisoning. The shim also tests a
host failure **after the Rust callback reports success but inside the actual SQL
transaction**: a fixed test-only storage facade invokes the callback and throws
before commit. Records/cursor roll back, cleanup runs, and Rust returns Host error,
not false commit success. No request provides arbitrary code or storage facade.

A successful operation binds `u64::MAX` (`18446744073709551615`) as an **exact SQL
TEXT parameter**, never a Number. This does not define production sequence storage
or numeric ordering.

### Same-wrapper panic, poison, and continuation checks

The default driver runs a panic through the **same generic FnOnce wrapper and
operation closure used for ordinary errors/success**, selecting the Shim boundary.
The callback writes a record prefix and a changed cursor, then panics with
`panic=abort`. Actual DO SQL rolls back. The shim observes RuntimeError, poisons the
instance, and rethrows across the **uncaught import**, out of the Rust root export.

Two `AtomicU32` counters instrument Rust continuation: the first statement after
the shim import returns, and the first statement after the generic transaction
returns. Their pointers are obtained **before** the trap using safe `as_ptr()` and
guarded exports. After the trap, JS reads them only via `DataView` over exported
Wasm memory. Observed counters stayed `import_returns: 4, transaction_returns: 7`
before and after panic: neither Rust continuation ran. Only plain-JS SQL/memory
reads occur afterward, not another call into the poisoned Wasm.

Every generated JS function (including init, pointer getters, comparison and new
exports) is invoked through the same `guardedRustEntry` function. After poison,
the test attempts each of the seven current generated functions and proves the
JS guard rejects **before invocation**. New generated function names are covered
by the runtime enumeration. The test then stops the process, constructs a fresh
workerd/Wasm instance, verifies persisted state and reset counters, and commits
another Rust operation. There is no automatic same-process recovery.

The root shim export accepts/returns `JsValue`. This is important: the generated
`Result<String>` comparison export has a `finally` that calls Wasm `free`, which
could reenter a poisoned instance during trap propagation. Generated code for the
shim export has no argument-string allocations, `catch`, or `finally/free` cleanup;
the driver checks this shape before running. Generated bindings are not patched.
The comparison string-return export remains restricted to **normal nonpanic** tests.

For the earlier separate one-shot, non-generic panic comparison, run:

```sh
nix shell nixpkgs#nodejs nixpkgs#python3 -c python3 design_checks/worker_sql/bridge_check.py --legacy-trap
```

Both the default generic-shim and legacy modes pass on the pinned runtime. This
provides actual panic SQL rollback and fail-closed entry-boundary evidence for
this configuration. It is **not** normal Rust unwinding, destructor recovery, safe
poisoned-instance reuse, an automatic workerd/Cloudflare isolate disposal mechanism,
or a complete production Store implementation. Availability after poisoning needs
an explicit host disposition/reset mechanism; only full process restart is tested.
Do not claim that `catch_unwind` or a catch import makes arbitrary aborts recoverable.

The RuntimeError classifier is tested against these ordinary Result sentinel errors
and the intentional Rust abort. Classification of arbitrary exceptions or hostile
callbacks is not established; the callback/storage facade are fixed trusted probe
code, not untrusted Core input. Other untested cases include destructor panics,
nested transactions, retained callbacks, allocation failure, commit failure,
disk-full errors, memory corruption, and different trap-producing host imports.
Consequently the entire production Store `Err/panic` contract and recovery strategy
still need integration and adversarial validation; this probe does not weaken it.

Final verification on the pinned Linux x86-64 runtime: default and legacy runtime
assertions, standalone Clippy with warnings denied, and fmt passed. There are no
production keys, signatures, bootstrap paths, or remote management policies here.

## References

- [Cloudflare SQLite-backed DO storage API](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/)
- [workerd configuration schema](https://github.com/cloudflare/workerd/blob/main/src/workerd/server/workerd.capnp)
  (`enableSql`, `durableObjectStorage.localDisk`, `--socket-fd`, directory service;
  upstream main is mutable, so the pinned npm binary remains this probe's runtime).
- [workerd repository](https://github.com/cloudflare/workerd)

The configuration's localDisk support is experimental in upstream workerd. This is
an intentionally disposable local runtime test, not a backup or deployment format.
