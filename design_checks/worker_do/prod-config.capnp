using Workerd = import "/workerd/workerd.capnp";

# Local smoke test for the deployable Worker under deploy/cloudflare.
const config :Workerd.Config = (
  services = [
    (name = "hub", worker = (
      modules = [
        (name = "worker.js", esModule = embed "../../deploy/cloudflare/src/worker.js"),
        (name = "pkg/host-api.js", esModule = embed "../../deploy/cloudflare/src/pkg/host-api.js"),
        (name = "pkg/harmion_worker_do.js", esModule = embed "../../deploy/cloudflare/src/pkg/harmion_worker_do.js"),
        (name = "pkg/harmion_worker_do_bg.wasm", wasm = embed "../../deploy/cloudflare/src/pkg/harmion_worker_do_bg.wasm")
      ],
      compatibilityDate = "2026-11-01",
      bindings = [
        (name = "NODE", durableObjectNamespace = "Node"),
        (name = "CLUSTER", text = "0101010101010101010101010101010101010101010101010101010101010101"),
        (name = "NODE_ID", text = "0303030303030303030303030303030303030303030303030303030303030303"),
        (name = "EPOCH", text = "0303030303030303030303030303030303030303030303030303030303030303"),
        (name = "SEED", text = "0303030303030303030303030303030303030303030303030303030303030303"),
        (name = "ORIGIN", text = "https://server.example"),
        (name = "ADMIN_TOKEN", text = "local-test-admin-token"),
        (name = "BOOTSTRAP_TOKEN", text = "local-test-bootstrap-token"),
        (name = "SYNC_INTERVAL_MS", text = "3600000")
      ],
      durableObjectNamespaces = [
        (className = "Node", uniqueKey = "harmion-do-prod-smoke-v1", enableSql = true)
      ],
      durableObjectStorage = (localDisk = "storage")
    )),
    (name = "assets", assets = (
      path = "../../deploy/cloudflare/public",
      config = "{\"run_worker_first\":[\"/state\",\"/peer/*\",\"/admin/*\",\"/provision\",\"/capabilities\",\"/overview\",\"/members\",\"/peers\",\"/provision-status\",\"/registration-code\",\"/internal/*\"],\"not_found_handling\":\"404-page\"}",
      userWorker = (name = "hub")
    )),
    (name = "storage", disk = (writable = true)),
    (name = "internet", network = (allow = []))
  ],
  sockets = [(name = "probe", http = (), service = "assets")]
);
