using Workerd = import "/workerd/workerd.capnp";

# Local experiment only. The driver supplies an already-bound 127.0.0.1 socket
# via --socket-fd and an ephemeral directory via --directory-path.
const config :Workerd.Config = (
  services = [
    (name = "probe", worker = (
      modules = [(name = "probe.js", esModule = embed "probe.js")],
      compatibilityDate = "2026-11-01",
      bindings = [(name = "PROBE", durableObjectNamespace = "SqlProbe")],
      durableObjectNamespaces = [
        (className = "SqlProbe", uniqueKey = "harmion-local-sql-probe-v1", enableSql = true)
      ],
      durableObjectStorage = (localDisk = "storage")
    )),
    (name = "storage", disk = (writable = true)),
    # Override the implicit public-internet capability. No outbound networking.
    (name = "internet", network = (allow = []))
  ],
  sockets = [(name = "probe", http = (), service = "probe")]
);
