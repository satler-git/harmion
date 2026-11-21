# Local DO bridge probe

Local-only workerd harness for the `harmion-worker` Durable Object bridge. It is
not a deployable Worker: the admin/bootstrap tokens are fixed local test values,
Cloudflare Access does not exist here, and there is no TLS termination.

## Build and run

From the repository root, inside `nix develop` (node, python3, rust, and the
wasm32 target are provided by the project flake):

```sh
cargo build -p harmion-worker --target wasm32-unknown-unknown --release --offline
nix shell nixpkgs#wasm-bindgen-cli -c wasm-bindgen --target web \
  --out-dir design_checks/worker_do/pkg --out-name harmion_worker_do \
  target/wasm32-unknown-unknown/release/harmion_worker.wasm
python3 design_checks/worker_do/check.py
```

`pkg/` is generated and ignored. The driver reuses the pinned workerd binary from
[`../worker_sql`](../worker_sql) and binds a loopback socket with temporary disk.
It provisions the DO through the host-only bootstrap route, runs a real signed
Peer v2 challenge/exchange (`MembershipStatus`, `Accept`, `Page`), restarts
workerd, and verifies the durable log and the read-only admin views.

The Rust client is a normal Cargo example (`crates/worker/examples/probe_client.rs`)
and is not part of the deployed crate.
