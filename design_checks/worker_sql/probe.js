// Host SQL/runtime probe ONLY: these are not Harmion records or auth rules.
const fail = (message) => { throw new Error(message); };
const check = (condition, message) => { if (!condition) fail(message); };
const bytes = (value) => new TextEncoder().encode(value).buffer;
const hex = (value) => Array.from(new Uint8Array(value), b => b.toString(16).padStart(2, "0")).join("");

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.hostname !== "127.0.0.1" || request.headers.has("Origin") ||
        request.method !== "GET" || !["/run", "/state"].includes(url.pathname) || url.search) {
      return new Response("Local test routes only", { status: 403 });
    }
    return env.PROBE.get(env.PROBE.idFromName("one-local-test-object")).fetch(request);
  }
};

export class SqlProbe {
  constructor(state) {
    this.storage = state.storage;
    this.sql = state.storage.sql;
    this.storage.transactionSync(() => {
      this.sql.exec("CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)");
      this.sql.exec("INSERT OR IGNORE INTO metadata VALUES ('node_id', ?)", crypto.randomUUID());
      this.sql.exec("INSERT OR IGNORE INTO metadata VALUES ('activations', '0')");
      this.sql.exec("UPDATE metadata SET value=CAST(CAST(value AS INTEGER)+1 AS TEXT) WHERE key='activations'");
      this.sql.exec("CREATE TABLE IF NOT EXISTS records (id TEXT PRIMARY KEY, bytes BLOB NOT NULL)");
      this.sql.exec("CREATE TABLE IF NOT EXISTS cursor (singleton INTEGER PRIMARY KEY, through_value TEXT NOT NULL)");
      this.sql.exec("INSERT OR IGNORE INTO cursor VALUES (1, '0')");
      this.sql.exec("CREATE TABLE IF NOT EXISTS integers (source TEXT PRIMARY KEY, integer_value INTEGER NOT NULL, text_value TEXT NOT NULL, blob_value BLOB NOT NULL)");
    });
  }

  one(sql, ...args) { return this.sql.exec(sql, ...args).one(); }
  cursor() { return this.one("SELECT through_value AS value FROM cursor").value; }
  count() { return this.one("SELECT count(*) AS count FROM records").count; }
  state() {
    return {
      node_id: this.one("SELECT value FROM metadata WHERE key='node_id'").value,
      activations: this.one("SELECT value FROM metadata WHERE key='activations'").value,
      cursor: this.cursor(),
      records: this.count(),
      integers: this.sql.exec("SELECT source, CAST(integer_value AS TEXT) AS exact_integer, text_value, hex(blob_value) AS blob_hex FROM integers ORDER BY source").toArray(),
    };
  }

  accept(entries, through, injection = "") {
    return this.storage.transactionSync(() => {
      for (const [id, value] of entries) {
        const previous = this.sql.exec("SELECT bytes FROM records WHERE id=?", id).toArray();
        if (previous.length) {
          check(hex(previous[0].bytes) === hex(bytes(value)), "same ID with different bytes");
        } else {
          this.sql.exec("INSERT INTO records VALUES (?, ?)", id, bytes(value));
        }
      }
      if (injection === "after-prefix") fail("injected prefix failure");
      this.sql.exec("UPDATE cursor SET through_value=? WHERE singleton=1", through);
      if (injection === "after-cursor") fail("injected cursor failure");
      return "committed";
    });
  }

  run() {
    check(this.count() === 0 && this.cursor() === "0", "run requires fresh temporary storage");
    const passed = [];
    for (const injection of ["after-prefix", "after-cursor"]) {
      let caught = false;
      try { this.accept([["prefix", "first valid entry"], ["second", "second valid entry"]], "2", injection); }
      catch (error) { caught = error.message.includes("injected"); }
      check(caught && this.count() === 0 && this.cursor() === "0", `${injection} did not roll back page+cursor`);
      passed.push(`thrown-${injection}-rollback`);
    }
    check(this.accept([["kept", "original bytes"]], "1") === "committed", "callback return lost");
    this.accept([["kept", "original bytes"]], "2");
    check(this.count() === 1 && this.cursor() === "2", "duplicate grew record log");
    passed.push("same-id-same-bytes-no-new-record");
    let mismatch = false;
    try { this.accept([["prefix", "must roll back"], ["kept", "different bytes"]], "3"); }
    catch (error) { mismatch = error.message.includes("different bytes"); }
    check(mismatch && this.count() === 1 && this.cursor() === "2", "mismatch did not roll back prefix+cursor");
    passed.push("same-id-mismatch-prefix-rollback");

    // Returning an Error is NOT throwing one: demonstrate the bridge hazard.
    const returned = this.storage.transactionSync(() => {
      this.sql.exec("INSERT INTO records VALUES ('returned-error', ?)", bytes("committed"));
      return new Error("ordinary callback value");
    });
    check(returned instanceof Error && this.count() === 2, "ordinary error return behavior changed");
    this.sql.exec("DELETE FROM records WHERE id='returned-error'");
    passed.push("returned-error-value-commits-not-rollback");

    const values = ["9007199254740993", "-9007199254740993", "9223372036854775807", "-9223372036854775808"];
    this.storage.transactionSync(() => {
      for (const value of values) {
        this.sql.exec("INSERT INTO integers VALUES (?, CAST(? AS INTEGER), ?, ?)", value, value, value, bytes(value));
      }
    });
    const state = this.state();
    for (const row of state.integers) {
      check(row.source === row.exact_integer && row.source === row.text_value, "64-bit text cast lost precision");
      check(row.blob_hex.toLowerCase() === hex(bytes(row.source)), "BLOB bytes lost precision");
    }
    passed.push("signed-64bit-cast-text-and-blob-exact");
    // Report the unsafe Number path without treating its rounded value as exact.
    const number = this.one("SELECT integer_value AS value FROM integers WHERE source='9007199254740993'").value;
    check(typeof number === "number" && !Number.isSafeInteger(number), "expected unsafe Number observation");
    return { passed, unsafe_number_observation: String(number), state };
  }

  async fetch(request) {
    try {
      const result = new URL(request.url).pathname === "/run" ? this.run() : this.state();
      // Explicit flush before reporting durable outcomes. This does not model
      // cloud replication, power-loss durability or a Rust trap/unwind.
      await this.storage.sync();
      return Response.json(result);
    } catch (error) {
      return Response.json({ error: error.message }, { status: 500 });
    }
  }
}
