# Cloudflare Worker hub

Deployable shape of the peer-only Harmion replica. It serves signed-HTTPS Peer
v2, a read-only admin UI/API, host-only provisioning, and scheduled outbound
sync. App data, Query, Subscribe and OAuth are intentionally unavailable.

Nothing here has been deployed from this repository. The local workerd probe in
`design_checks/worker_do` exercises the same Worker/DO code; `wrangler deploy`
and real Cloudflare Access/secrets/cost have not been run.

## Build

Inside `nix develop` with `wasm-bindgen-cli` (0.2.130) available:

```sh
nix shell nixpkgs#wasm-bindgen-cli -c bash deploy/cloudflare/build.sh
```

Outputs `src/pkg/` (wasm + bindings) and `public/` (the Svelte admin UI).

## Configure

1. Set `CLUSTER`, `NODE_ID`, `EPOCH` and `ORIGIN` in [wrangler.toml](wrangler.toml).
   `ORIGIN` must be the canonical `https://` origin that the node's membership
   entry and registration code use; there must be no redirect to it.
2. Generate a new 32-byte Ed25519 seed for this node and keep it only in
   Cloudflare secrets. Never reuse a seed from another node.
3. Put the secrets:

```sh
wrangler secret put SEED
wrangler secret put ADMIN_TOKEN
wrangler secret put BOOTSTRAP_TOKEN
```

4. Deploy and attach the custom domain/route. Cloudflare terminates TLS; the
   origin must match the certificate hostname.

## Cloudflare Access (recommended outer layer)

Create Zero Trust applications so the UI/admin are SSO-protected while peers can
still reach the protocol:

- `hub.example/` → Allow the owner (email/IdP), covers `/` and `/assets/*` and
  `/admin/*`.
- `hub.example/peer` → Bypass (or a service-token policy), so `/peer/v2/*` is
  reachable by peers that only hold node keys.
- Leave `/provision` and `/internal/*` outside Access; they are protected by the
  bootstrap token and the app-layer bearer. Prefer blocking them at the WAF too.

The Worker still enforces `Authorization: Bearer ADMIN_TOKEN` for admin and
internal routes; Access alone is not the app's authentication.

## Provisioning

The host-only `POST /provision` requires `x-harmion-bootstrap: $BOOTSTRAP_TOKEN`
and a JSON manifest:

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
deliberate identical repair, then remove it. Rotate or remove
`BOOTSTRAP_TOKEN` after provisioning.

Generate this manifest from an active cluster instead of composing it by hand.
On the native founder, approve the Worker's `registration-code` first, then:

```sh
harmion --node /path/to/node provision-manifest worker-registration.json \
  --out manifest.json
```

The command requires ManageNodes, refuses a registration that is not Active,
and emits only public membership material (own member, root anchor,
root-first admission chain, active contacts) — never `admin.secret`. Its output
is the JSON above. Post it with `x-harmion-bootstrap`; then sync normally.

The Worker's own `harmion-peer:v1:` code is available at
`POST /admin/registration-code` (bearer) so an owner can approve it from a
native admin.

## Verify after deploy

```sh
curl -s https://hub.example/state
curl -s -X POST -H "Authorization: Bearer $ADMIN_TOKEN" \
  https://hub.example/admin/provision-status
```

Then approve the registration code on a native node, confirm the Worker becomes
Active, and check that a native sync round pushes/pulls records. `/internal/alarm`
(bearer) forces one scheduled round; `SYNC_INTERVAL_MS` sets the normal alarm
interval (default 300000 ms, 0 disables).

## Operations

- Backups: another full node holds the records; a new Worker can be provisioned
  with a new identity and receive history from a native node. DO time-travel
  restore is not documented here and has not been tested.
- Rotation: retire this node from the native admin, then provision a new Worker
  with a new seed/Node ID.
- Monitoring: `/state` for liveness, Cloudflare logs/metrics for 5xx/429, and a
  native sync status for staleness. The Worker admin API has no rate limiter of
  its own.
- Cost: Durable Object SQL storage, requests, and outbound fetch egress. Actual
  limits/quota have not been measured.

## Not implemented

- Real deployment, Access policy application, secret distribution and TLS
  verification against Cloudflare.
- Alarm minimum-interval behavior, crash-during-commit, DO eviction, multi-region.
- Bootstrap/manifest CLI and automatic secret rotation.
