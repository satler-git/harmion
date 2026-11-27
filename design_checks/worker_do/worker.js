// Local-only workerd harness for the Harmion DO bridge. Not a deployable
// Worker: secrets are plain local bindings and no Cloudflare Access exists.
import * as rust from "./pkg/harmion_worker_do.js";
import wasmModule from "./pkg/harmion_worker_do_bg.wasm";

rust.initSync({ module: wasmModule });

const PEER = "application/vnd.harmion.https-peer-v2";
const JSON_TYPE = "application/json";
const ADMIN_PATHS = [
  "/capabilities",
  "/overview",
  "/members",
  "/peers",
  "/provision-status",
  "/registration-code",
];

const REQUIRED_ENV = ["CLUSTER", "NODE_ID", "EPOCH", "SEED", "ORIGIN", "ADMIN_TOKEN"];
const HEX_ENV = ["CLUSTER", "NODE_ID", "EPOCH", "SEED"];

// Local probe: unlike the production host, a short test ADMIN_TOKEN is allowed.
function configurationProblem(env) {
  const missing = REQUIRED_ENV.filter((name) => {
    const value = env[name];
    return typeof value !== "string" || value.length === 0 || value.startsWith("REPLACE_");
  });
  const invalid = HEX_ENV.filter((name) => !/^[0-9a-f]{64}$/.test(env[name] ?? ""));
  try {
    const origin = new URL(env.ORIGIN);
    if (origin.protocol !== "https:" || origin.pathname !== "/" || origin.search || origin.hash) {
      invalid.push("ORIGIN");
    }
  } catch {
    invalid.push("ORIGIN");
  }
  return missing.length || invalid.length ? { missing, invalid } : null;
}

async function identityFingerprint(env) {
  const material = [env.CLUSTER, env.NODE_ID, env.EPOCH, env.SEED, env.ORIGIN].join("|");
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(material),
  );
  return Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
}

// Admin JSON times are decimal microseconds, like the native admin API.
function micros(value) {
  const millis = Number(value);
  return Number.isFinite(millis) && millis > 0 ? String(millis * 1000) : null;
}

export default {
  async fetch(request, env) {
    globalThis.__harmionPeers = env.LEAF ? { "leaf.internal": env.LEAF } : {};
    return env.NODE.get(env.NODE.idFromName("main")).fetch(request);
  },
};

export class Node {
  constructor(state, env) {
    this.state = state;
    this.storage = state.storage;
    this.env = env;
    this.state.blockConcurrencyWhile(async () => {
      const interval = Number(this.env.SYNC_INTERVAL_MS ?? "0");
      if (interval > 0 && (await this.state.storage.getAlarm()) === null) {
        await this.state.storage.setAlarm(Date.now() + interval);
      }
    });
  }

  async alarm() {
    try {
      await this.scheduledSync();
    } catch (error) {
      console.error("scheduled sync failed", error);
    }
    const interval = Number(this.env.SYNC_INTERVAL_MS ?? "0");
    if (interval > 0) {
      await this.state.storage.setAlarm(Date.now() + interval);
    }
  }

  async scheduledSync() {
    const now = Date.now();
    const storedSchedule = await this.storage.get("sync/schedule");
    const schedule =
      storedSchedule && typeof storedSchedule === "object" && !Array.isArray(storedSchedule)
        ? storedSchedule
        : {};
    const peers = await rust.outbound_peers(
      this.env.CLUSTER,
      this.env.NODE_ID,
      this.env.EPOCH,
    );
    const due = [];
    for (const peer of peers) {
      const entry = schedule[peer] ?? { failures: 0, next: 0 };
      if (entry.next <= now) due.push(peer);
    }
    if (due.length === 0) return { results: [] };
    const results = await rust.outbound_sync_list(
      this.env.CLUSTER,
      this.env.NODE_ID,
      this.env.EPOCH,
      this.env.SEED,
      this.env.ALLOW_INTERNAL === "1",
      due.join(","),
    );
    for (const result of results) {
      const entry = schedule[result.peer] ?? { failures: 0, next: 0 };
      entry.last_at = now;
      entry.last_ok = result.ok;
      if (result.ok) {
        entry.failures = 0;
        entry.next = now + Number(this.env.SYNC_INTERVAL_MS ?? "300000");
      } else {
        entry.failures += 1;
        entry.next = now + Math.min(1800000, 1000 * 2 ** Math.min(entry.failures, 10));
      }
      schedule[result.peer] = entry;
    }
    await this.storage.put("sync/schedule", schedule);
    await this.storage.sync();
    return { results };
  }

  async fetch(request) {
    const url = new URL(request.url);
    if (request.method === "GET" && url.pathname === "/state") {
      return Response.json({ ok: true });
    }
    if (request.method !== "POST") {
      return new Response("method not allowed", { status: 405 });
    }
    const body = new Uint8Array(await request.arrayBuffer());
    globalThis.__harmionDo = this;
    try {
      if (url.pathname === "/provision") return await this.provision(request, body);
      if (url.pathname === "/internal/sync") return await this.sync(request, body);
      if (url.pathname === "/internal/alarm") {
        const authorization = request.headers.get("authorization") ?? "";
        if (
          !this.env.ADMIN_TOKEN ||
          authorization !== `Bearer ${this.env.ADMIN_TOKEN}`
        ) {
          return Response.json({ error: "unauthorized" }, { status: 401 });
        }
        return Response.json(await this.scheduledSync());
      }
      if (url.pathname.startsWith("/leaf/")) {
        const headers = new Headers(request.headers);
        headers.delete("host");
        const target =
          "https://leaf.internal" + url.pathname.slice("/leaf".length) + url.search;
        return this.env.LEAF.fetch(
          new Request(target, { method: request.method, headers, body }),
        );
      }
      if (url.pathname === "/peer/v2/challenge" || url.pathname === "/peer/v2/exchange") {
        return await this.peer(request, url, body);
      }
      if (url.pathname.startsWith("/admin/")) return await this.admin(request, url.pathname.slice(7));
      if (ADMIN_PATHS.includes(url.pathname)) return await this.admin(request, url.pathname.slice(1));
      return new Response("not found", { status: 404 });
    } finally {
      globalThis.__harmionDo = undefined;
    }
  }

  async peer(request, url, body) {
    const result = rust.peer_handle(
      this.env.CLUSTER,
      this.env.NODE_ID,
      this.env.EPOCH,
      this.env.SEED,
      this.env.ORIGIN,
      url.pathname,
      request.headers.get("host") ?? url.host,
      request.headers.has("origin"),
      Date.now() * 1000,
      body,
    );
    if (url.pathname === "/peer/v2/exchange" && result.status === 200 && result.data) {
      await this.recordInbound();
    }
    await this.storage.sync();
    return new Response(result.body, {
      status: result.status,
      headers: { "content-type": PEER },
    });
  }

  async recordInbound() {
    try {
      const now = Date.now();
      const previous = await this.storage.get("sync/last_inbound");
      if (
        previous &&
        typeof previous === "object" &&
        Number(previous.at) > now - 60000
      ) {
        return;
      }
      await this.storage.put("sync/last_inbound", { at: now });
    } catch (error) {
      console.error("last inbound metadata failed", error);
    }
  }

  authorized(request) {
    const authorization = request.headers.get("authorization") ?? "";
    return (
      typeof this.env.ADMIN_TOKEN === "string" &&
      this.env.ADMIN_TOKEN.length > 0 &&
      authorization === `Bearer ${this.env.ADMIN_TOKEN}`
    );
  }

  async hostStatus() {
    try {
      const problem = configurationProblem(this.env);
      const provisioned = await this.storage.get("provisioned_state");
      const hasProvisioning = Boolean(provisioned && typeof provisioned === "object");
      const currentIdentity = hasProvisioning
        ? await identityFingerprint(this.env)
        : null;
      const alarm = await this.storage.getAlarm();
      const lastInbound = await this.storage.get("sync/last_inbound");
      const storedSchedule = await this.storage.get("sync/schedule");
      const schedule =
        storedSchedule &&
        typeof storedSchedule === "object" &&
        !Array.isArray(storedSchedule)
          ? storedSchedule
          : {};
      const interval = Number(this.env.SYNC_INTERVAL_MS ?? "0");
      return Response.json(
        {
          config: {
            ok: !problem,
            missing: problem?.missing ?? [],
            invalid: problem?.invalid ?? [],
          },
          provisioning: {
            provisioned: hasProvisioning,
            provisioned_at: hasProvisioning
              ? micros(provisioned.provisioned_at)
              : null,
            identity_matches:
              hasProvisioning && provisioned.identity === currentIdentity,
            bootstrap_enabled:
              typeof this.env.BOOTSTRAP_TOKEN === "string" &&
              this.env.BOOTSTRAP_TOKEN.length >= 32 &&
              this.env.ALLOW_PROVISION === "1",
            allow_reprovision: this.env.ALLOW_REPROVISION === "1",
          },
          sync: {
            interval_ms: String(Number.isFinite(interval) && interval > 0 ? interval : 0),
            alarm_next_at: micros(alarm),
            last_inbound_at:
              lastInbound && typeof lastInbound === "object"
                ? micros(lastInbound.at)
                : null,
            schedule: Object.entries(schedule)
              .sort(([left], [right]) => left.localeCompare(right))
              .map(([node, entry]) => ({
                node,
                failures: String(entry?.failures ?? 0),
                next: micros(entry?.next),
                last_at: micros(entry?.last_at),
                last_ok: typeof entry?.last_ok === "boolean" ? entry.last_ok : null,
              })),
          },
        },
        { headers: { "cache-control": "no-store" } },
      );
    } catch (error) {
      console.error("host status failed", error);
      return Response.json(
        { error: "unavailable" },
        { status: 503, headers: { "cache-control": "no-store" } },
      );
    }
  }

  async admin(request, operation) {
    const authorization = request.headers.get("authorization") ?? "";
    const provided = authorization.startsWith("Bearer ") ? authorization.slice(7) : "";
    if (operation === "host-status") {
      if (!this.authorized(request)) {
        return Response.json({ error: "unauthorized" }, { status: 401 });
      }
      return this.hostStatus();
    }
    const result = rust.admin_handle(
      this.env.CLUSTER,
      this.env.NODE_ID,
      this.env.EPOCH,
      this.env.SEED,
      this.env.ORIGIN,
      Date.now() * 1000,
      operation,
      this.env.ADMIN_TOKEN,
      provided,
    );
    return new Response(result.body, {
      status: result.status,
      headers: { "content-type": JSON_TYPE, "cache-control": "no-store" },
    });
  }

  async sync(request, body) {
    const authorization = request.headers.get("authorization") ?? "";
    if (!this.env.ADMIN_TOKEN || authorization !== `Bearer ${this.env.ADMIN_TOKEN}`) {
      return Response.json({ error: "unauthorized" }, { status: 401 });
    }
    const peer = new TextDecoder().decode(body).trim();
    const result = await rust.outbound_sync(
      this.env.CLUSTER,
      this.env.NODE_ID,
      this.env.EPOCH,
      this.env.SEED,
      peer,
      this.env.ALLOW_INTERNAL === "1",
    );
    await this.storage.sync();
    return Response.json(result);
  }

  async provision(request, body) {
    if (
      (await this.storage.get("provisioned")) &&
      this.env.ALLOW_REPROVISION !== "1"
    ) {
      return Response.json({ error: "already_provisioned" }, { status: 409 });
    }
    const bootstrap = request.headers.get("x-harmion-bootstrap") ?? "";
    if (!this.env.BOOTSTRAP_TOKEN || bootstrap !== this.env.BOOTSTRAP_TOKEN) {
      return Response.json({ error: "unauthorized" }, { status: 401 });
    }
    const manifest = new TextDecoder().decode(body);
    rust.provision(
      this.env.CLUSTER,
      this.env.NODE_ID,
      this.env.EPOCH,
      this.env.SEED,
      manifest,
    );
    await this.storage.put("provisioned_state", {
      identity: await identityFingerprint(this.env),
      provisioned_at: Date.now(),
    });
    await this.storage.put("provisioned", true);
    await this.storage.sync();
    return Response.json({ configured: true });
  }
}
