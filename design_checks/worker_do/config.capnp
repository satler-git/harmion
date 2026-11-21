using Workerd = import "/workerd/workerd.capnp";

# Local-only probe. The driver supplies a loopback socket and temporary disk.
# Secrets below are fixed test values; never deploy this configuration.
# `hub` is the externally reachable node; `leaf` is only reachable through the
# hub's service binding, modelling an outbound-only peer.
const config :Workerd.Config = (
  services = [
    (name = "hub", worker = (
      modules = [
        (name = "worker.js", esModule = embed "worker.js"),
        (name = "pkg/host-api", esModule = embed "host-api.js"),
        (name = "pkg/harmion_worker_do.js", esModule = embed "pkg/harmion_worker_do.js"),
        (name = "pkg/harmion_worker_do_bg.wasm", wasm = embed "pkg/harmion_worker_do_bg.wasm")
      ],
      compatibilityDate = "2026-11-01",
      bindings = [
        (name = "NODE", durableObjectNamespace = "Node"),
        (name = "LEAF", service = "leaf"),
        (name = "CLUSTER", text = "0101010101010101010101010101010101010101010101010101010101010101"),
        (name = "NODE_ID", text = "0303030303030303030303030303030303030303030303030303030303030303"),
        (name = "EPOCH", text = "0303030303030303030303030303030303030303030303030303030303030303"),
        (name = "SEED", text = "0303030303030303030303030303030303030303030303030303030303030303"),
        (name = "ORIGIN", text = "https://server.example"),
        (name = "ADMIN_TOKEN", text = "local-test-admin-token"),
        (name = "BOOTSTRAP_TOKEN", text = "local-test-bootstrap-token"),
        (name = "ALLOW_INTERNAL", text = "1"),
        (name = "SYNC_INTERVAL_MS", text = "3600000")
      ],
      durableObjectNamespaces = [
        (className = "Node", uniqueKey = "harmion-do-hub-v1", enableSql = true)
      ],
      durableObjectStorage = (localDisk = "storage")
    )),
    (name = "leaf", worker = (
      modules = [
        (name = "worker.js", esModule = embed "worker.js"),
        (name = "pkg/host-api", esModule = embed "host-api.js"),
        (name = "pkg/harmion_worker_do.js", esModule = embed "pkg/harmion_worker_do.js"),
        (name = "pkg/harmion_worker_do_bg.wasm", wasm = embed "pkg/harmion_worker_do_bg.wasm")
      ],
      compatibilityDate = "2026-11-01",
      bindings = [
        (name = "NODE", durableObjectNamespace = "Node"),
        (name = "CLUSTER", text = "0101010101010101010101010101010101010101010101010101010101010101"),
        (name = "NODE_ID", text = "0404040404040404040404040404040404040404040404040404040404040404"),
        (name = "EPOCH", text = "0404040404040404040404040404040404040404040404040404040404040404"),
        (name = "SEED", text = "0404040404040404040404040404040404040404040404040404040404040404"),
        (name = "ORIGIN", text = "https://leaf.internal"),
        (name = "ADMIN_TOKEN", text = "local-test-admin-token"),
        (name = "BOOTSTRAP_TOKEN", text = "local-test-bootstrap-token"),
        (name = "ALLOW_INTERNAL", text = "0")
      ],
      durableObjectNamespaces = [
        (className = "Node", uniqueKey = "harmion-do-leaf-v1", enableSql = true)
      ],
      durableObjectStorage = (localDisk = "storage")
    )),
    (name = "assets", assets = (
      path = "ui",
      config = "{\"run_worker_first\":[\"/state\",\"/peer/*\",\"/admin/*\",\"/provision\",\"/capabilities\",\"/overview\",\"/members\",\"/peers\",\"/provision-status\",\"/registration-code\",\"/internal/*\",\"/leaf/*\"],\"not_found_handling\":\"404-page\"}",
      userWorker = (name = "hub")
    )),
    (name = "storage", disk = (writable = true)),
    (name = "internet", network = (allow = []))
  ],
  sockets = [(name = "probe", http = (), service = "assets")]
);
