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
const ADMIN_PATHS = [
  "/capabilities",
  "/overview",
  "/members",
  "/peers",
  "/provision-status",
  "/registration-code",
];

export default {
  async fetch(request, env) {
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
      if (url.pathname === "/internal/alarm") return await this.alarmNow(request);
      if (url.pathname === "/peer/v2/challenge" || url.pathname === "/peer/v2/exchange") {
        return await this.peer(request, url, body);
      }
      if (url.pathname.startsWith("/admin/")) {
        return await this.admin(request, url.pathname.slice("/admin/".length));
      }
      if (ADMIN_PATHS.includes(url.pathname)) {
        return await this.admin(request, url.pathname.slice(1));
      }
      return new Response("not found", { status: 404 });
    } finally {
      globalThis.__harmionDo = undefined;
    }
  }

  authorized(request) {
    const authorization = request.headers.get("authorization") ?? "";
    return Boolean(this.env.ADMIN_TOKEN) && authorization === `Bearer ${this.env.ADMIN_TOKEN}`;
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
    if ((await this.storage.get("provisioned")) && this.env.ALLOW_REPROVISION !== "1") {
      return Response.json({ error: "already_provisioned" }, { status: 409 });
    }
    const bootstrap = request.headers.get("x-harmion-bootstrap") ?? "";
    if (!this.env.BOOTSTRAP_TOKEN || bootstrap !== this.env.BOOTSTRAP_TOKEN) {
      return Response.json({ error: "unauthorized" }, { status: 401 });
    }
    const manifest = new TextDecoder().decode(body);
    rust.provision(this.env.CLUSTER, this.env.NODE_ID, this.env.EPOCH, this.env.SEED, manifest);
    await this.storage.put("provisioned", true);
    await this.storage.sync();
    return Response.json({ configured: true });
  }
}
