// Production Worker for a Harmion peer-only replica.
//
// Cloudflare Access is the outer layer for the UI/admin paths; the app-layer
// bearer below is still mandatory. Peer v2, /provision and /internal/* are not
// covered by Access and are protected by signed frames, the bootstrap token and
// the admin bearer respectively.
import * as rust from "./pkg/harmion_worker_do.js";
import wasmModule from "./pkg/harmion_worker_do_bg.wasm";

rust.initSync({ module: wasmModule });

const PEER = "application/vnd.harmion.https-peer-v2";
const JSON_TYPE = "application/json";
// Reject oversized requests before buffering a body into the isolate. The Rust
// frame decoder has the same limits, but reaching it requires the bytes to be
// allocated first; an unauthenticated peer path must stay bounded at the edge.
// crates/protocol/src/peer.rs: MAX_PEER_BYTES = 4 MiB page + 8 KiB envelope.
const MAX_PEER_BYTES = 4 * 1024 * 1024 + 8 * 1024;
const MAX_FRAME_BYTES = MAX_PEER_BYTES + 8 * 1024;
const BODY_LIMITS = new Map([
  ["/peer/v2/challenge", 8 * 1024],
  ["/peer/v2/exchange", MAX_FRAME_BYTES],
  ["/provision", 1024 * 1024],
  ["/internal/sync", 4 * 1024],
  ["/internal/alarm", 0],
]);
const ADMIN_BODY_LIMIT = 64 * 1024;
const ADMIN_PATHS = [
  "/capabilities",
  "/overview",
  "/members",
  "/peers",
  "/provision-status",
  "/registration-code",
];

// Workers Assets ignores `_headers`, and `/` is behind Cloudflare Access, so
// the Worker serves the document itself and attaches the same headers as the
// native admin server (apps/admin/src/server.rs). API and peer responses are
// JSON/binary and do not need a document CSP.
const DOCUMENT_PATHS = new Set(["/", "/index.html"]);
const DOCUMENT_HEADERS = {
  "content-security-policy":
    "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
  "referrer-policy": "no-referrer",
  "x-content-type-options": "nosniff",
  "permissions-policy": "camera=(self), microphone=(), geolocation=()",
  "cache-control": "no-store",
};

async function documentResponse(request, env) {
  const asset = await env.ASSETS.fetch(new URL("/index.html", request.url));
  const headers = new Headers(asset.headers);
  for (const [name, value] of Object.entries(DOCUMENT_HEADERS)) {
    headers.set(name, value);
  }
  return new Response(asset.body, { status: asset.status, headers });
}

function bodyLimit(path) {
  if (BODY_LIMITS.has(path)) return BODY_LIMITS.get(path);
  if (path.startsWith("/admin/") || ADMIN_PATHS.includes(path)) {
    return ADMIN_BODY_LIMIT;
  }
  return -1;
}

async function readBoundedBody(request, limit) {
  const declared = request.headers.get("content-length");
  if (declared !== null) {
    const length = Number(declared);
    if (!Number.isSafeInteger(length) || length < 0 || length > limit) {
      return null;
    }
  }
  if (limit === 0 || request.body === null) return new Uint8Array(0);
  const reader = request.body.getReader();
  const chunks = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > limit) {
      await reader.cancel().catch(() => {});
      return null;
    }
    chunks.push(value);
  }
  const body = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return body;
}

function constantTimeEqual(left, right) {
  if (left.length !== right.length) return false;
  let difference = 0;
  for (let index = 0; index < left.length; index += 1) {
    difference |= left.charCodeAt(index) ^ right.charCodeAt(index);
  }
  return difference === 0;
}

const REQUIRED_ENV = ["CLUSTER", "NODE_ID", "EPOCH", "SEED", "ORIGIN", "ADMIN_TOKEN"];
const HEX_ENV = ["CLUSTER", "NODE_ID", "EPOCH", "SEED"];

function configurationProblem(env) {
  const missing = REQUIRED_ENV.filter((name) => {
    const value = env[name];
    return typeof value !== "string" || value.length === 0 || value.startsWith("REPLACE_");
  });
  const invalid = HEX_ENV.filter((name) => !/^[0-9a-f]{64}$/.test(env[name] ?? ""));
  if (typeof env.ADMIN_TOKEN !== "string" || env.ADMIN_TOKEN.length < 32) {
    invalid.push("ADMIN_TOKEN");
  }
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

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (request.method === "GET" && DOCUMENT_PATHS.has(url.pathname)) {
      return documentResponse(request, env);
    }
    // A changed NODE_ID intentionally gets a fresh Durable Object instead of
    // failing forever with IdentityMismatch on the previous node's storage.
    return env.NODE.get(env.NODE.idFromName(`node:${env.NODE_ID ?? "unconfigured"}`)).fetch(
      request,
    );
  },
};

export class Node {
  constructor(state, env) {
    this.state = state;
    this.storage = state.storage;
    this.env = env;
    // The Rust bridge reaches back into the active DO through
    // globalThis.__harmionDo. Serialize handlers so an awaiting request cannot
    // overwrite that context while another handler resumes after an await.
    this.tail = Promise.resolve();
    this.state.blockConcurrencyWhile(async () => {
      const interval = Number(this.env.SYNC_INTERVAL_MS ?? "0");
      if (interval > 0 && (await this.state.storage.getAlarm()) === null) {
        await this.state.storage.setAlarm(Date.now() + interval);
      }
    });
  }

  async fetch(request) {
    const url = new URL(request.url);
    // Liveness must not sit behind a long sync round.
    if (request.method === "GET" && url.pathname === "/state") {
      return Response.json({ ok: true });
    }
    if (request.method === "GET" && url.pathname === "/ready") {
      const problem = configurationProblem(this.env);
      if (problem) return Response.json({ ok: false, ...problem }, { status: 503 });
      const state = await this.storage.get("provisioned_state");
      if (!state || typeof state !== "object") {
        return Response.json({ ok: false, provisioned: false }, { status: 503 });
      }
      const current = await identityFingerprint(this.env);
      if (state.identity !== current) {
        return Response.json(
          { ok: false, provisioned: true, identity_mismatch: true },
          { status: 503 },
        );
      }
      return Response.json({ ok: true, provisioned: true });
    }
    if (request.method !== "POST") {
      return new Response("method not allowed", { status: 405 });
    }
    const limit = bodyLimit(url.pathname);
    if (limit < 0) return new Response("not found", { status: 404 });
    // Do not buffer provision bodies from clients that cannot present the
    // bootstrap token; /provision is reachable without Cloudflare Access.
    if (url.pathname === "/provision") {
      const bootstrap = request.headers.get("x-harmion-bootstrap") ?? "";
      if (typeof this.env.BOOTSTRAP_TOKEN !== "string"
        || this.env.BOOTSTRAP_TOKEN.length < 32
        || !constantTimeEqual(bootstrap, this.env.BOOTSTRAP_TOKEN)) {
        return Response.json({ error: "unauthorized" }, { status: 401 });
      }
    }
    // Read a bounded body before taking the serialization slot: a slow client
    // must not hold up every other operation on this Durable Object.
    const body = await readBoundedBody(request, limit);
    if (body === null) {
      return new Response(JSON.stringify({ error: "payload_too_large" }), {
        status: 413,
        headers: { "content-type": JSON_TYPE },
      });
    }
    return this.runExclusive(() => this.handle(request, url, body));
  }

  runExclusive(task) {
    const run = this.tail.then(task, task);
    this.tail = run.then(
      () => {},
      () => {},
    );
    return run;
  }

  async handle(request, url, body) {
    const previous = globalThis.__harmionDo;
    globalThis.__harmionDo = this;
    try {
      if (url.pathname === "/provision") return await this.provision(request, body);
      if (url.pathname === "/internal/sync") return await this.sync(request, body);
      if (url.pathname === "/internal/alarm") return await this.alarmNow(request);
      if (url.pathname === "/peer/v2/challenge" || url.pathname === "/peer/v2/exchange") {
        return await this.peer(request, url, body);
      }
      if (url.pathname.startsWith("/admin/")) {
        return await this.admin(request, url.pathname.slice("/admin/".length));
      }
      return await this.admin(request, url.pathname.slice(1));
    } finally {
      globalThis.__harmionDo = previous;
    }
  }

  authorized(request) {
    const authorization = request.headers.get("authorization") ?? "";
    return Boolean(this.env.ADMIN_TOKEN)
      && constantTimeEqual(authorization, `Bearer ${this.env.ADMIN_TOKEN}`);
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
    await this.storage.sync();
    return new Response(result.body, {
      status: result.status,
      headers: { "content-type": PEER },
    });
  }

  async admin(request, operation) {
    const authorization = request.headers.get("authorization") ?? "";
    const provided = authorization.startsWith("Bearer ") ? authorization.slice(7) : "";
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
    if (!this.authorized(request)) {
      return Response.json({ error: "unauthorized" }, { status: 401 });
    }
    const peer = new TextDecoder().decode(body).trim();
    const result = await rust.outbound_sync(
      this.env.CLUSTER,
      this.env.NODE_ID,
      this.env.EPOCH,
      this.env.SEED,
      peer,
      false,
    );
    await this.storage.sync();
    return Response.json(result);
  }

  async alarmNow(request) {
    if (!this.authorized(request)) {
      return Response.json({ error: "unauthorized" }, { status: 401 });
    }
    return Response.json(await this.scheduledSync());
  }

  async alarm() {
    return this.runExclusive(async () => {
      const previous = globalThis.__harmionDo;
      globalThis.__harmionDo = this;
      try {
        await this.scheduledSync();
      } catch (error) {
        console.error("scheduled sync failed", error);
      } finally {
        globalThis.__harmionDo = previous;
      }
      const interval = Number(this.env.SYNC_INTERVAL_MS ?? "0");
      if (interval > 0) {
        await this.state.storage.setAlarm(Date.now() + interval);
      }
    });
  }

  async scheduledSync() {
    const now = Date.now();
    const schedule = (await this.storage.get("sync/schedule")) ?? {};
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
      false,
      due.join(","),
    );
    for (const result of results) {
      const entry = schedule[result.peer] ?? { failures: 0, next: 0 };
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

  async provision(request, body) {
    // Authenticate before revealing provisioning state, and require an explicit
    // per-deployment opt-in so a stale token cannot provision a new replica.
    const bootstrap = request.headers.get("x-harmion-bootstrap") ?? "";
    if (typeof this.env.BOOTSTRAP_TOKEN !== "string"
      || this.env.BOOTSTRAP_TOKEN.length < 32
      || !constantTimeEqual(bootstrap, this.env.BOOTSTRAP_TOKEN)) {
      return Response.json({ error: "unauthorized" }, { status: 401 });
    }
    if (this.env.ALLOW_PROVISION !== "1") {
      return Response.json({ error: "provisioning_disabled" }, { status: 403 });
    }
    if ((await this.storage.get("provisioned_state")) && this.env.ALLOW_REPROVISION !== "1") {
      return Response.json({ error: "already_provisioned" }, { status: 409 });
    }
    const manifest = new TextDecoder().decode(body);
    rust.provision(this.env.CLUSTER, this.env.NODE_ID, this.env.EPOCH, this.env.SEED, manifest);
    // One write makes the provisioned marker and its identity atomic.
    await this.storage.put("provisioned_state", {
      identity: await identityFingerprint(this.env),
      provisioned_at: Date.now(),
    });
    await this.storage.sync();
    return Response.json({ configured: true });
  }
}
