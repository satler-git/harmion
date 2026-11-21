// Host boundary exposed to the Rust/Wasm bridge. The Worker sets
// globalThis.__harmionDo for the duration of one request.
function host() {
  const value = globalThis.__harmionDo;
  if (!value) throw new Error("no active Durable Object host");
  return value;
}

export function sqlExec(sql, params) {
  const storage = host().storage;
  const rows = [];
  for (const row of storage.sql.exec(sql, ...(params ?? []))) {
    const values = Array.isArray(row)
      ? row
      : typeof row?.[Symbol.iterator] === "function"
        ? Array.from(row)
        : Object.values(row);
    rows.push(
      values.map((value) => (value instanceof ArrayBuffer ? new Uint8Array(value) : value)),
    );
  }
  return rows;
}

export function transaction(callback) {
  return host().storage.transactionSync(() => callback());
}

export function randomBytes() {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return bytes;
}

// Outbound signed-HTTPS POST. Only canonical https origins are reachable; the
// redirect policy is manual so a 3xx is rejected instead of followed.
export async function fetchPeer(url, body) {
  const parsed = new URL(url);
  if (parsed.protocol !== "https:") {
    throw new Error("peer origin must be https");
  }
  const response = await fetch(url, {
    method: "POST",
    body,
    headers: { "content-type": "application/vnd.harmion.https-peer-v2" },
    redirect: "manual",
    signal: AbortSignal.timeout(15000),
  });
  if (response.status >= 300 && response.status < 400) {
    throw new Error("redirect rejected");
  }
  return { status: response.status, body: new Uint8Array(await response.arrayBuffer()) };
}
