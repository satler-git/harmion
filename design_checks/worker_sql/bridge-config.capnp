using Workerd = import "/workerd/workerd.capnp";

# Local-only Rust/Wasm bridge experiment. Driver supplies loopback socket/temp disk.
const config :Workerd.Config = (
  services = [
    (name = "probe", worker = (
      modules = [
        (name = "bridge-probe.js", esModule = embed "bridge-probe.js"),
        (name = "bridge.js", esModule = embed "rust_bridge/pkg/bridge.js"),
        (name = "bridge-shim", esModule = embed "bridge-shim.js"),
        (name = "bridge_bg.wasm", wasm = embed "rust_bridge/pkg/bridge_bg.wasm")
      ],
      compatibilityDate = "2026-11-01",
      bindings = [(name = "PROBE", durableObjectNamespace = "BridgeProbe")],
      durableObjectNamespaces = [
        (className = "BridgeProbe", uniqueKey = "harmion-local-rust-bridge-v1", enableSql = true)
      ],
      durableObjectStorage = (localDisk = "storage")
    )),
    (name = "storage", disk = (writable = true)),
    (name = "internet", network = (allow = []))
  ],
  sockets = [(name = "probe", http = (), service = "probe")]
);
