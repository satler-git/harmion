# Cloudflare Worker hub

Deployable shape of the peer-only Harmion replica. It serves signed-HTTPS Peer
v2, a read-only admin UI/API, host-only provisioning, and a durability target for
native-initiated sync. App data, Query, Subscribe and OAuth are intentionally
unavailable.

The local workerd probes in `design_checks/worker_do` exercise the same Worker/DO
code. `wrangler deploy` has been exercised separately; Cloudflare Access policy,
real quota/cost measurements and DO time-travel restore remain operator tasks.

## Build

From the repository root, build inside `nix develop`. The script builds the
admin UI through `nix build .#admin-ui` and the wasm bridge with the dev shell's
`wasm-bindgen-cli` and clang:

```sh
nix develop --command bash deploy/cloudflare/build.sh
```

`wrangler deploy` intentionally does not run the build: the build needs the
Nix dev shell and the wasm target. Re-run the command above after changing Rust
or the admin UI, then deploy.

Outputs `src/pkg/` (wasm + bindings) and `public/` (the Svelte admin UI). The
Worker serves `/` and `/index.html` itself and adds the CSP and other security
headers; Workers Assets ignores `_headers` files, and `/` is behind Cloudflare
Access, so the headers are attached in `src/worker.js` rather than by the
Assets layer.

## Configure

Run the remaining commands from `deploy/cloudflare` (or pass
`--cwd deploy/cloudflare` to every wrangler call).

1. Write the actual `CLUSTER`, `NODE_ID`, `EPOCH` and `ORIGIN` into
   `wrangler.toml`. `ORIGIN` must be the canonical `https://` origin that the
   node's membership entry and registration code use; there must be no redirect
   to it. This repository's `wrangler.toml` contains the public identity for the
   `hub.satler.dev` deployment; replace all four values together for another
   deployment. `/ready` returns 503 for placeholders or malformed values. Do not
   rely on `--var` for identity values: unless every deploy repeats all of them,
   the next deploy silently restores the file's values.
2. `CLUSTER` is the **native founder's** cluster ID. `NODE_ID`, `EPOCH` and
   `SEED` are fresh 32-byte hex values for this Worker. Never reuse a seed from
   another node.
3. Set the custom domain in `wrangler.toml` (`routes`) and make it match
   `ORIGIN` exactly.
4. Put the secrets. `ADMIN_TOKEN` and `BOOTSTRAP_TOKEN` must be independently
   generated 32-byte random values (for example
   `head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n'`), not passwords or
   reused strings. Use exactly the same 64-hex form for `SEED`, `NODE_ID`, `EPOCH`
   and `CLUSTER`; `/ready` rejects anything else.

```sh
printf '%s' "$SEED" | wrangler secret put SEED
printf '%s' "$ADMIN_TOKEN" | wrangler secret put ADMIN_TOKEN
printf '%s' "$BOOTSTRAP_TOKEN" | wrangler secret put BOOTSTRAP_TOKEN
```

5. Deploy. `ALLOW_PROVISION=1` is required only for the first provisioning
   window; `wrangler.toml` defaults it to `"0"` so a stale token cannot
   provision a new replica.

```sh
# 1. Set ALLOW_PROVISION = "1" in wrangler.toml for the first deployment only.
wrangler deploy
# 2. Provision once (below).
# 3. Set ALLOW_PROVISION = "0" in wrangler.toml and deploy again, or delete the
#    BOOTSTRAP_TOKEN secret and keep the temporary override out of the repo.
wrangler deploy
```

`wrangler.toml` sets `SYNC_INTERVAL_MS = "0"` by default. Native initiates sync;
native is not a SignedHttps v2 server, so a Worker-originated alarm cannot reach
native peers. Set it to a non-zero value only for Worker-to-Worker clusters.
`/internal/alarm` (bearer) can still force one round manually.

## Cloudflare Access and authentication

Initial deployment uses the app-layer boundary explicitly:

- `/admin/*`, `/capabilities`, `/overview`, `/members`, `/peers`,
  `/provision-status`, `/registration-code` and `/internal/*` require
  `Authorization: Bearer ADMIN_TOKEN`.
- `/peer/v2/*` is public by design and authenticated by signed frames, pinned
  node keys and durable challenges.
- `/provision` requires the one-time bootstrap token **and** `ALLOW_PROVISION=1`.
- `/state` is an unauthenticated liveness probe; `/ready` reports missing or
  placeholder configuration and whether the node has been provisioned.

Cloudflare Access is enabled for `hub.satler.dev` as defense in depth:

- `hub.satler.dev` (`/`, `/assets/*`, `/admin/*`, `/capabilities`) uses a
  One-time PIN identity provider and allows `mitsclatw24@mbr.nifty.com`
  (session 8h).
- Bypass applications cover `/peer`, `/state`, `/ready`, `/provision` and
  `/internal`. Native sync and monitoring cannot hold Access cookies; the
  app-layer bearer/protocol checks remain.
- `/admin/*` now returns a 302 to the Access login unless the browser has an
  Access session. Scripted admin calls need an Access service token or the
  browser cookie; the bearer token alone no longer reaches the Worker at the
  edge.
- Access verifies the user at the edge; the Worker does not verify
  `Cf-Access-Jwt-Assertion` itself.

Verify the split with:

```sh
curl -s -o /dev/null -w '%{http_code}\n' https://hub.satler.dev/state  # 200
curl -s -o /dev/null -w '%{http_code}\n' https://hub.satler.dev/ready  # 200
curl -s -o /dev/null -w '%{http_code}\n' https://hub.satler.dev/       # 302
```

Access setup and DNS changes require an API token with **Access: Apps and
Policies: Edit** and **Zone: DNS: Edit**. The wrangler OAuth token cannot make
those changes.

## Provisioning

The host-only `POST /provision` requires `x-harmion-bootstrap: $BOOTSTRAP_TOKEN`,
a valid configuration and a JSON manifest:

```json
{
  "own": {
    "node": "<64 hex>",
    "signing_key": "<64 hex>",
    "binding": { "transport": "signed_https", "key": "<64 hex>" },
    "endpoint": "https://hub.example"
  },
  "root": "<hex HMC certificate>",
  "evidence": ["<hex HMC certificate>", "..."],
  "contacts": [ { "...": "same shape as own" } ]
}
```

Provisioning is idempotent for the same manifest and fails on a conflicting
root/identity/key. After the first successful apply the route returns
`409 already_provisioned`; re-apply only with `ALLOW_REPROVISION=1` for a
deliberate identical repair, then remove it. After provisioning:

1. Set `ALLOW_PROVISION = "0"` in `wrangler.toml` and redeploy. Keep the same
   identity values in the file; do not redeploy with only an `ALLOW_PROVISION`
   override.
2. Delete the `BOOTSTRAP_TOKEN` secret (`wrangler secret delete BOOTSTRAP_TOKEN`)
   or rotate it.
3. Confirm `/ready` returns 200 and `/admin/provision-status` reports the expected
   identity.

Generate the manifest from an active cluster instead of composing it by hand.
On the native founder, approve the Worker's `registration-code` first, then:

```sh
harmion --node /path/to/node provision-manifest worker-registration.json \
  --out manifest.json
```

The command requires ManageNodes, refuses a registration that is not Active,
and emits only public membership material (own member, root anchor,
root-first admission chain, active contacts) — never `admin.secret`. Its output
is the JSON above. Post it with `x-harmion-bootstrap`; then sync from native.

The Worker's own `harmion-peer:v1:` code is available at
`POST /admin/registration-code` (bearer) so an owner can approve it from a
native admin.

## Verify after deploy

```sh
curl -s https://hub.satler.dev/state          # liveness only
curl -s https://hub.satler.dev/ready          # 503 until configured+provisioned
curl -s -X POST -H "Authorization: Bearer $ADMIN_TOKEN" \
  https://hub.satler.dev/admin/provision-status
```

Then approve the registration code on a native node, confirm the Worker becomes
Active, and run a native sync round. The Worker stores accepted records and
membership evidence; native remains the initiator.

If a Durable Object was provisioned by an older build, it has no
`provisioned_state` fingerprint. Re-apply the same manifest once with
`ALLOW_REPROVISION=1` (and `ALLOW_PROVISION=1`) to record it, or use a fresh
`NODE_ID`. Changing `ORIGIN` changes the fingerprint too; re-register the node
after an origin change.

## CI deploy token

GitHub Actions deploys with a scoped Cloudflare API token stored in the
`cloudflare-prod` Environment. Create a **custom token** with:

| Scope | Permission | Why |
|---|---|---|
| Account | Workers Scripts: Edit | Upload/deploy the Worker and apply Durable Object migrations |
| Zone `satler.dev` | Workers Routes: Edit | Keep the `hub.satler.dev` route/custom domain |
| Zone `satler.dev` | Zone: Read | Required by wrangler for zone lookup in some versions |

- `Workers Scripts: Edit` covers the deployed Durable Object script; there is no
  separate Durable Objects permission in the current token permission list.
- `DNS: Edit` is only needed if wrangler creates the custom domain's DNS record.
  `hub.satler.dev` already exists; do not grant DNS by default.
- Do not grant Access, account settings, billing, KV or R2. Access/DNS changes
  use the separate operator token described above.
- Set the token as `CLOUDFLARE_API_TOKEN` in the GitHub Environment
  `cloudflare-prod`, and `CLOUDFLARE_ACCOUNT_ID` as the repository/environment
  variable. The Worker's runtime secrets (`SEED`, `ADMIN_TOKEN`,
  `BOOTSTRAP_TOKEN`) are not part of this token and must not be set by CI.

## Operations

- Rotation: a changed `NODE_ID` maps to a new Durable Object (`node:<NODE_ID>`),
  so a new Worker identity starts with empty storage instead of failing with
  `IdentityMismatch`. Retire the old node from native, deploy the new identity,
  provision it from an active native node, then delete the old Worker/DO when no
  longer needed. `CLUSTER` must not change for an existing cluster.
- Backups: another full node holds the records. The native node directory is
  backed up separately (see `docs/linux-service.md`); DO time-travel restore has
  not been tested.
- Monitoring: `/state` for liveness, `/ready` for configuration/provisioning,
  Cloudflare logs/metrics for 5xx/429, and native `sync-status` for staleness.
  The admin API has no rate limiter of its own; protect it with Access/WAF if
  exposed beyond the owner.
- Cost: Durable Object SQL storage, requests and outbound fetch egress. Free-plan
  limits and actual quota usage must be checked in the Cloudflare dashboard.

## Not implemented

- Applying an Access policy, automated secret rotation, DO time-travel restore.
- Crash-during-commit, DO eviction, multi-region, and Worker-to-native scheduled
  push (native is not a SignedHttps v2 server).
