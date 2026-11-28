# Signed HTTPS Peer v2 — design draft

**Status: design proposal plus a locally tested implementation; not a deployed feature.**
2026-11-20, updated 2026-11-21. All new wire tags, routes, budgets and interface
names below remain proposed; freeze them only after the implementation gates
below. Since the first draft, the following are implemented and tested locally
(see [implementation review](../docs/records/implementation-review.md) R33–R41): the
canonical envelope codec with independent fixtures, durable signed-identity pins
with provenance/migration and cut-excluded-issuer tests, HMC v2 `SignedHttps`
binding with a dual reader and registration v4, a bounded durable challenge
store with CAS consumption and clock/quota rules, the native Peer v2 entry and
outbound URL transport over real TLS, a `harmion-worker` host/KV boundary that
compiles for `wasm32-unknown-unknown`, and a **peer-only** DO node `Store`
(records, cursors, membership, peers, identity pins) over the synchronous SQL
boundary, host-only provisioning, a read-only Worker admin API, the wasm-bindgen
DO bridge over `transactionSync`/`sql.exec`, and the Worker fetch/assets/routing
wiring (exercised in local workerd with a real Rust Peer client). **Still not
implemented**: real Cloudflare deployment, Access policy, secret distribution,
TLS termination, WebPKI/redirect/SSRF behavior against a real network, DO
startup history verification, and Android/Windows. Passing local and memory-host tests is not a
production or availability claim.


## 1. Scope and invariants

The owner-approved target is a permanent Cloudflare Workers / SQLite-backed
Durable Object replica, initially **Peer-only**, including signed membership
admission/retirement/dismissal evidence gossip. Future remote management is a
separate App API credential/grant project. Peer possession never grants App
ManageNodes. Do not copy native admin secrets to Cloudflare.

One cluster maps to one persistently identified DO node. The deploy owner
explicitly provisions its node signing seed, Node ID, Cluster ID, common pinned
membership root, admission chain, and trusted contacts. Unconfigured nodes fail
closed; no public request initializes identity or picks a root. Reapplying an
identical deployment manifest is idempotent; conflicting initialization is an
owner-visible failure, never key regeneration. Secret delivery/storage and backup
recovery require a separate deployment procedure before production use.

Linux precedes Android, then Windows. Android initiates bounded push/pull rounds
when its OS permits; no reachable Android listener, permanent session, timer or
Tailnet gateway is required. Cloudflare is a peer, not membership's authority.

Use normal authenticated HTTPS **and** Ed25519 node signatures. TLS protects
confidentiality/routing and ordinary Internet server authentication; signatures
prove the separately pinned Harmion identity. Neither alone is a membership
authorization decision. A challenge authenticates the cloud node before the
client sends record/certificate bytes, and authenticates the requester before the
cloud allocates challenge state. An exchange proof is required before the cloud
serves or accepts actual Peer data.

## 2. Existing contracts and narrow changes

Read with [Peer v1](peer-v1.md), [membership v1](membership-v1.md),
[Cloudflare design](../docs/core/cloudflare.md), and
[architecture](../docs/core/architecture.md). The latter contains earlier high-level
management wording; this draft specifically proposes Peer-only initial exposure.

- Reuse exact `encode_peer_request` / `encode_peer_response` HPQ/HPS v1 bytes
  from [peer codec](../crates/protocol/src/peer.rs). Envelope v2 is a **transport**
  version, not a new record format or changed cursor semantics.
- Preserve original record/certificate bytes and existing per-page limits,
  fixed-high rounds, durable cursor semantics and all-or-nothing acceptance.
- [membership handler](../crates/node/src/membership.rs) already owns the
  transaction containing current authorization, receipt, log and cursor updates.
  Do not split it, or require a wholesale `handle_in_transaction` refactor.
- Add a distinct proven identity kind for signed HTTPS, minimal key lookup and
  exhaustive binding checks. Never synthesize a TLS certificate or iroh handshake
  from a signature. The existing host-supplied proven-key boundary remains trusted
  host code, not a public `authenticated: true` header.
- [MembershipSigner](../crates/node/src/membership_state.rs) currently signs only
  HMC Statements and omits Debug deliberately. A later implementation needs typed
  `sign_https_challenge_request`, `sign_https_challenge`, `sign_https_exchange`,
  and `sign_https_response` operations using its established Ed25519 seed, or an
  equivalent constrained signer port. Do not expose a network arbitrary-signing
  oracle or repurpose `sign(Statement)` with fake HMC actions. Use strict Ed25519
  verification as current membership does. Native seed reuse does not mean TLS
  certificates are Ed25519 membership keys.

## 3. Trusted key resolution: identity is not current authority

A request contains Node ID, **not a requester-selected public key or certificate
chain**. Resolve `(cluster, node) -> unique trusted signing key + binding` from
local verified state before verifying its signature. Never use a key embedded in
an unauthenticated request, arbitrary self-signed HMC bytes, URL, HTTP header,
Cloudflare account, or routing-table entry as the trust anchor.

The current `Status::Retired` / `Conflict` variants do not retain a Member;
`require_active` alone therefore cannot implement evidence authentication.
`tx.peer` is a routing/transport projection, not a signing-key authority. Add a
**durable local known-signing-identity registry**, queried by
`known_peer_identity(tx, node)`, separate from current membership grants:

1. Pin `(cluster, node, signing_key, binding, provenance)` only through explicit
   owner-verified bootstrap/registration or an **effectively authorized admission
   at acceptance**, under the existing membership evaluator. Commit automatic
   pin insertion with the accepted membership page/projection/cursor transaction.
   Provenance records the local owner action or original accepted evidence ID.
2. Never overwrite a pin or reuse its signing key for another Node ID. Retain
   pins after retirement/conflict/unconfirmed transitions: they prove possession
   for evidence transport, not current permission to serve data. Registry removal
   or replacement requires a separately specified owner recovery procedure.
3. A claim in `Membership.rooted` alone is **not** permission to insert a pin or
   poison an existing pin. A retired issuer can create a cut-excluded new Admit
   naming a victim Node ID with another key: its historical rootedness must not
   disable the victim's already pinned control-plane authentication. Ignore such
   ineffective claims for pin updates, while retaining original evidence under
   existing membership rules. Do not build ambiguity checks over all raw history.
   An actually effective conflicting admission still fails existing membership
   identity validation; never silently select one or mutate an existing pin.
4. Read the pin before signature verification, then recheck it and current
   membership in the operation's transaction. Unknown/target-only identities
   cannot authenticate. A local self-identity is not a remote trust root.

This registry is durable **authentication identity**, not a disposable cache and
not a source of membership grants. On old-DB migration, seed only from explicitly
pinned roots/recovery registrations and current effectively authorized identities
whose exact key/binding is validated. Never reconstruct missing pins from every
historical rooted Admit, or guess acceptance-time authority from receipt order.
Historical retired/contested peers lacking a safe old pin remain unknown until
explicit owner verification or valid effective evidence received through an
already known peer establishes the mapping. Their existing native transport may
still provide its separately validated evidence path; do not claim seamless v2
control authentication for every old DB. The exact migration/import/provenance
schema and any safe reconstruction of older acceptance-time pins remain an
implementation gate. Provision initial contact pins explicitly; that does not
make those contacts membership roots or bypass signed admission evaluation.

**Authorization matrix:**

| Proven identity / operation | Evidence commands (tags 4–7) | Data commands (tags 0–3) |
|---|---|---|
| Known durably pinned key, even retired/conflict/unconfirmed | Allowed subject to bounds and signed HMC validation | Only if both peers currently Active and pin matches effective identity |
| Unknown/unpinned key or mismatched proof | Deny without challenge allocation | Deny |

Evidence transfer neither approves the requester nor authorizes newly received
certificates. Existing rooted membership evaluation still decides their effect.
For data, check both local and remote Active state and immutable identity **in
the existing node transaction**, including when persisting pull responses.
Authenticate again/recheck the mapping at exchange time, not just challenge time.
A membership exchange may retire its own sender; it may still receive its bound
membership ACK after commit. That grants no subsequent data access.

## 4. Endpoint routing and HTTPS profile

Draft routes: `POST /peer/v2/challenge` and `POST /peer/v2/exchange` only.
Content-Type: `application/vnd.harmion.https-peer-v2`; no Content-Encoding,
query string, fragment, URL credentials or alternate path normalization. No App,
OAuth, browser CORS or remote-admin routes are exposed by this profile. Reject
Origin-bearing requests (native clients do not send Origin). Responses use
`Cache-Control: no-store`; no caching proxy may manufacture success.

Configure an owner-approved base origin `https://<dns-name>[:port]`, plus a
non-secret bounded route label for the cluster/DO if necessary. This draft uses
one origin with routing by the signed cluster/audience, not arbitrary path
prefixes. Route labels/DO names are lookup hints, not identities or secrets.
The Worker must map to the intended DO and the DO independently verifies cluster,
audience and signatures. Unknown routes must not create unbounded persistent DO
state. Preprovision/allowlist routing rather than initialize per arbitrary ID.

Clients validate the complete normal WebPKI chain, hostname, time and server-auth
purpose, and send SNI for the URL hostname. No `harmion.local` override, self-signed
bypass, leaf substitution or disabled validation. **Never follow redirects**, even
to another HTTPS URL. Treat every 3xx as failure; never forward Access headers or
envelopes. Owner-approved origins and an egress policy must prevent membership
endpoint gossip from becoming an arbitrary/private-address SSRF mechanism.

Define canonical origin bytes once with a standard URL parser: lowercase ASCII
IDNA DNS host, omit default `:443`, include nondefault numeric port, no trailing
slash. Draft excludes IP literals and noncanonical host spellings. Reject
ambiguous Host/authority, duplicate Host or conflicting proxy routing. Sign the
configured public origin, not attacker-supplied forwarded headers. Exact canonical
origin equality and exact ASCII route equality are required at both endpoints.
Changes of origin require explicit local owner route configuration; they do not
rotate Node ID/key or silently rewrite historical membership evidence. Send a
bounded descriptive non-secret User-Agent such as `harmion/0.1` for edge
interoperability; it is not an authentication input. Edge blocking, including an
HTML challenge/error, is a transport failure, never a reason to follow redirects,
disable WebPKI or leak credentials. Verify workers.dev versus custom-domain edge
behavior and origin configuration during deployment; this draft does not claim
those account-specific policies are controllable by application authentication.

## 5. Canonical binary envelope

Integers are unsigned big-endian; IDs, SHA-256 digests and random nonces are 32
bytes; Ed25519 signatures are 64 bytes. `B(s)` means u32 byte length followed by
exact bytes. No optional fields, maps, varints, trailing bytes or unknown tags.
UTF-8 fields must meet the bounded ASCII subsets here. Reject oversize lengths
before allocation. Hash raw canonical bytes, never JSON or reconstructed HTTP.

Every frame starts `HHT\0 | u32(2) | u8(kind)` followed by these fields in order.
All frames end with the sender's signature. Define `U(frame)` as all frame bytes
before that final signature, `H(frame)` as SHA-256 of the complete frame including
signature, and sign **ordinary Ed25519** over
`ASCII("harmion/https-peer/v2/") || ASCII(kind-name) || 0x00 || U(frame)`.
Kind names below are literal ASCII. Do not use Ed25519ph or signatures over an
unspecified prehash. Distinct kind domains prevent reflection and HMC reuse.

Common `C` = `cluster | sender_node | audience_node | B(origin)`.
IDs must be different; audience is a stable Node ID, never the URL/DO identifier.
No public-key field is present. Request payload class is u8: data=0, evidence=1.
Request method byte is 0=`POST`; any other value fails. Paths below are exact ASCII.

| Kind / domain name | Fields after prefix, before signature |
|---|---|
| 0 `challenge-request` (client → server) | C, method=0, B(`/peer/v2/challenge`), client_nonce, exchange_method=0, B(`/peer/v2/exchange`), payload_class, u32 inner_length, inner_digest |
| 1 `challenge` (server → client) | C (reversed sender/audience), method=0, B(`/peer/v2/challenge`), H(challenge-request), client_nonce, server_generation, challenge_nonce, u32 ttl_ms, exchange_method=0, B(`/peer/v2/exchange`), payload_class, u32 inner_length, inner_digest |
| 2 `exchange` (client → server) | C, method=0, B(`/peer/v2/exchange`), H(challenge-request), H(challenge), client_nonce, server_generation, challenge_nonce, payload_class, B(inner HPQ v1 bytes) |
| 3 `response` (server → client) | C (reversed sender/audience), method=0, B(`/peer/v2/exchange`), H(exchange), H(challenge), client_nonce, server_generation, challenge_nonce, u16 HTTP_status, u8 result_kind, B(result_bytes) |

The common origin is the same **server** origin in both directions. Generation
and nonces are independent CSPRNG values. The complete challenge need not travel
again in exchange: the server stores its exact hash and all bound fields. The
inner digest is SHA-256 of exact HPQ bytes; inner_length excludes envelope bytes.
The server recomputes both after bounded reading and verifies agreement with the
stored challenge before consumption. Inner cluster/node must equal envelope
cluster/sender; inner command must match payload_class. Inner HPS cluster/node
must equal response cluster/sender. No command is executed from a challenge.

`result_kind`: 0 = exact HPS v1 PeerResponse, 1 = transport error described below.
An authenticated application rejection such as `PeerReply::Rejected` remains
HPS, not a transport-error string. HTTP status and result kind are signed and
must agree with the actual HTTP response. For a successful HPS result use 200.

**Freshness rule:** the client creates a fresh client_nonce for every attempt,
retains exact outbound frame hashes only for live attempts, and accepts at most
one response matching the entire outstanding exchange/challenge context. It
must verify trusted server key, audience, origin, routes, all hashes/nonces,
status and inner message before advancing any cursor, confirmation, checkpoint or
last-success status. A valid old ACK with matching epoch/through is **not** proof
of this attempt. A signature alone, HTTP 200, or an unsigned error is not proof.
After process restart discard outstanding attempts and generate fresh nonces.
The server challenge proves the expected server key, but not data acceptance.

## 6. Challenge/exchange sequence and durable anti-replay

1. Client fixes the exact HPQ bytes and signs a metadata-only challenge request.
   It has not yet uploaded records or certificates. Server performs bounded
   parsing, route/identity lookup, strict signature verification and preliminary
   class authorization. Unknown/bad signatures allocate **zero** challenge rows.
2. In a short synchronous **transport-state transaction**, the server applies
   expiry/cap checks, creates a random challenge, and stores its bindings plus
   H(challenge-request), H(challenge), expiry and `unused`. Sign before inserting
   so exact response hash is stored; send the challenge only after commit. No
   data log, membership log or cursor changes. Duplicate challenge requests do
   not renew TTL or allocate a second row: return the identical stored challenge
   if still unused/live; otherwise require a fresh client_nonce. Index dedup by
   `(generation, requester, client_nonce)` and reject a different signed
   commitment using the same live nonce; do not count it as a new allocation.
   This is bounded idempotence, not permission to execute twice.
3. Client verifies the challenge signature against its already trusted server
   identity and verifies all echoed commitments. Then and only then uploads the
   signed exchange with the actual HPQ bytes. Cloudflare never initiates a data
   response merely because a challenge request was valid.
4. Server reads within size/deadline limits, verifies exchange signature, exact
   stored challenge bindings, hash/length/class, current trusted key and basic
   decode. In a short durable transaction, compare-and-swap **unused → used**.
   Commit this consumption **before** calling the node handler. The host must
   additionally await `storage.sync()` or a documented equivalent physical
   durability gate after logical transaction commit and **before dispatch**;
   `transactionSync` returning alone is not evidence of that gate. On flush
   failure do not dispatch. This await is outside both synchronous Store
   callbacks, not inside the existing node transaction. Concurrent
   identical exchanges have at most one winning caller. An absent, expired,
   consumed or generation-mismatched challenge never invokes the handler.
5. Only the winning invocation passes the proven signed-HTTPS identity to the
   existing `membership::handle`. That handler retains its separate, unchanged
   atomic node transaction: current membership checks, complete page validation,
   all records/projection/log entries and cursor together. No network await is
   introduced inside it. After commit, the host must complete its physical
   flush/output durability gate before signing or releasing an ACK/response;
   two `transactionSync` callbacks without yielding are not proof of independent
   durable commits. Sign only after that gate (or rollback for a rejection),
   bound to the exact exchange. A failed handler still consumes the
   challenge. No cached replay ACK is needed or proposed.

This intentionally gives **at most one dispatch per challenge, not exactly-once
transport delivery**. A crash after consumption but before handler invocation
loses an attempt without data effects. A crash during the node transaction leaves
all or none of the page. A crash after node commit but before response makes its
result uncertain, not failed: obtain a new challenge for Cursor or the same page,
then follow existing durable cursor reconciliation. Never roll back a used marker
because a later handler failed. Never commit a normal data prefix to make a
transport response possible. New-challenge resubmission is semantically safe
because the current Peer protocol validates cursor and record identity, not
because the transport guarantees exactly-once mutation.

Ordinary DO eviction/restart **preserves live durable challenges and the replay
generation**. Issuance and exchange are separate HTTP requests and must succeed
even if the runtime cold-starts between them. Generation changes only during
explicit identity/replay-store recovery, durably invalidating prior challenges;
it is not a process/session counter. A consumed row can be deleted: only an
existing live unused row can authorize execution; absence is rejection, never
reconstructed authorization.
Old signed challenge requests may obtain a new challenge after their dedup row is
gone, but cannot use it without a fresh exchange signature bound to that new
server nonce. This permits bounded state, not resurrection of old exchange proof.
A replayed authenticated challenge request can still consume its known peer's
quota; per-peer/global limits are required and this availability risk is explicit.

## 7. Bounded state, time, concurrency and costs

**Draft defaults, not frozen production guarantees:**

| Resource | Proposed bound |
|---|---|
| Challenge TTL | 60 seconds; client abandons after 45 seconds locally |
| Challenge metadata frame | 8 KiB maximum |
| Inner HPQ/HPS | Existing `MAX_PEER_BYTES` = 4 MiB + 8 KiB |
| Whole exchange/response | `MAX_PEER_BYTES` + 8 KiB; reject extra bytes |
| Origin / route bytes | 512 / 64 maximum; enrollment URL remains ≤2,048 |
| Challenge rows, unused + used + dedup combined | 8 per known requester, 256 per DO |
| Logical challenge-row storage budget | 1 MiB total; bounded row serialization, no inner bodies |
| New challenges | Workload-sized bounded token buckets; illustrative 120/minute per requester (burst 16), 480/minute per DO (burst 64), subject to multi-page tests |
| In-flight uploads / handlers | 2 per requester, 8 per DO; no unbounded queue |
| Request processing deadline | 15 seconds excluding client retry/backoff |
| HTTP headers | 32 headers, 16 KiB aggregate; bounded body even without length |

Reject at capacity, never evict a live unused row to admit an attacker. Expired
rows and old-generation rows are lazily collected in bounded batches; correctness
does not depend on an alarm executing on time. Used rows count against caps until
GC and expire at the original deadline; reclaim used rows promptly in bounded
batches before capacity checks rather than retaining all spent challenges for
60 seconds and accidentally imposing an eight-command/minute throughput cap.
Deleting a spent row makes later replay unknown/rejected; it never makes it unused.
Rate buckets are bounded to trusted
identities and durably bounded; per-process resets must not bypass DO quotas.
Fairness reservations may be needed for evidence recovery under data load.
Rate sizing must count **commands, pages and retries**, not just rounds. Even a
quiet membership+data round needs at least four commands; at a 30-second cadence
that already consumes eight challenges/minute before page transfer or retry.
Budget capacity from `rounds/minute × (base commands + upload/download pages +
retry allowance)`, aggregate across four concurrent peers, and reserve bounded
recovery/control capacity. Test long backlogs, small-page/high-latency operation,
ACK loss and four peers before selecting values; the illustrative numbers above
are neither throughput promises nor defaults to freeze. Retry exhaustion yields
backoff with durable progress, not an unbounded bypass.

TTL is replay hygiene, **not update causality or certificate issuance ordering**.
Persist absolute challenge expiry and an authentication-only time high-water in
the challenge store. Within an active runtime use monotonic elapsed time so a
backward/stalled wall clock cannot extend challenges; advance the durable auth
clock as max(observed wall time, previous auth time + monotonic elapsed). Never
derive update IDs, seq, epochs or membership cuts from it. Ordinary activation
loads the same replay generation/rows and reconciles against trusted host wall
time; it must not refresh TTL or replace a challenge merely due to cold start.
If observed wall time is behind the persisted auth high-water, **fail closed for
issuance and exchange** until it catches up or the owner performs explicit
clock/replay-store recovery; do not move high-water backward. A detected reversal
within a runtime likewise blocks operations rather than extending validity.
Forward jumps can expire attempts early. On suspension use host elapsed-time
information if guaranteed; otherwise recheck durable expiry/high-water on resume,
without resetting the replay generation. Missing/corrupt time or persistence
fails closed. Exact host clock/monotonic guarantees, including undetectably
stalled time across eviction, must be established before production: bounded
one-use CAS remains mandatory even when expiry cannot supply a perfect real-time
age guarantee. A time anomaly is an availability fault, not new update causality.
Arbitrary storage snapshot rollback is outside ordinary restart guarantees:
restoring old rows/generation under the same identity is unsafe and requires the
explicit restore/new-identity procedure before public serving.

App authentication does **not** stop unauthenticated bandwidth/CPU floods, TLS
costs, Worker invocation charges or routing abuse before DO authentication.
Streaming size limits, edge rate/WAF policies and deployment budget controls are
additional operational defenses, not claims of zero-cost public exposure.
Cloudflare Access Service Auth is an optional outer layer with separate,
per-deployment secrets; passing it does not replace node signatures or membership.
Tailnet membership is neither required nor inferred from an Access token.

## 8. Errors and secret handling

Before a trusted requester signature is verified, use a generic bounded unsigned
400/403/413/415/429/503 as appropriate, with no membership/key diagnostics or
challenge allocation. A malformed/unauthenticated error is never trusted as a
remote state transition. Unconfigured nodes return generic 503, never bootstrap.

After valid exchange authentication, return a signed kind-3 error whenever safe,
binding H(exchange) even when the challenge cannot be used. Error result bytes:
`u16 code | u32 retry_after_ms`; no arbitrary message, stack, SQL, keys, body or
cursor. Draft closed code set:

| Code | Meaning / HTTP status / client action |
|---|---|
| 1 InvalidEnvelope | 400; repair encoding, no confirmation |
| 2 NotAuthorized | 403; identity/current membership denied; stop data |
| 3 ChallengeUnavailable | 409; absent/used/expired/generation mismatch; fresh challenge and cursor reconciliation |
| 4 BindingMismatch | 400; wrong audience/origin/commitment; stop, investigate |
| 5 UnsupportedVersion | 400; upgrade, never downgrade silently |
| 6 Capacity | 429; bounded signed retry hint, jitter/backoff |
| 7 StoreUnavailable | 503; outcome uncertain, fresh challenge and cursor reconciliation |
| 8 InternalFailure | 500; outcome uncertain, same reconciliation |

An identity/route mismatch should usually stay generic unsigned rather than sign
for an unintended audience. Do not sign arbitrary untrusted megabytes to explain
a failure. Challenge-route authenticated errors need a separate small error frame:
propose kind=4, domain `challenge-error`, fields `C, method=0,
B(/peer/v2/challenge), H(challenge-request), client_nonce, u16 HTTP_status,
u16 code, u32 retry_after_ms`, followed by signature. It carries no challenge or
Peer payload and allocates no replay row. Clients may use verified errors for
backoff/diagnostics only, never as a success. Unknown error codes fail closed.
Unsigned proxy/Access/timeout responses remain transport failures. Even signed
StoreUnavailable/InternalFailure do not prove that no page committed.

Do not log signing seeds, OS/Worker secret handles, Access client secret, App
credentials, authorization headers, complete envelopes, challenge nonces, record
bodies or membership payloads. URLs contain no secrets. Redact at HTTP client,
Worker, DO, tracing and CLI error layers, not only display. Prefer bounded local
correlation IDs, route aliases, coarse error codes, counters and timings; Node ID
and cluster metadata also deserve privacy-conscious logging. Do not expose a
signer through Debug/serialization or include environment dumps in errors.

## 9. Membership binding and compatibility rollout

Do **not** add a meaning to HMC v1 tag 0, encode signing keys as fake DER, or put
an HTTPS URL into the old IP:port field expecting old clients to understand it.
Current HMC v1 knows only TLS DER tag=0 and iroh Endpoint ID tag=1, and rejects
unknown versions/tags. Proposed explicit **HMC v2**:

- Same HMC magic/common fields, version=2. Preserve actions and limits.
- Binding tag 0 = unchanged native TLS DER; tag 1 = unchanged iroh key;
  **tag 2 = SignedHttps([u8; 32])**, carrying the exact node signing public key.
  Enforce binding key == Member.signing_key; this repeats the existing key for
  explicit typed binding identity, not a second requester-chosen credential.
  Binding uniqueness compares this key, not a unit SignedHttps tag: a retired
  cloud node and its newly admitted replacement have distinct bindings. Never
  compare only the transport discriminant. Endpoint is the canonical
  owner-approved HTTPS base origin from section 4.
- New readers support original HMC v1 bytes/signatures/IDs unchanged and HMC v2.
  Do not rewrite pinned roots or historical certificates to v2. Signature and
  certificate ID cover the actual version bytes. New v2 Retire/Dismiss statements
  retain existing causality/retained-set semantics; version is not a timestamp.
- New registration JSON v4 explicitly represents `transport: signed_https`,
  Node ID, signing key, origin and original common root/admission evidence.
  Native SQL binding tags/schema migration must be explicit and checked. Unknown
  variants fail closed; do not reinterpret them as a default transport.

An upgraded native node with an existing TLS/iroh identity may **initiate** this
profile and prove the same durably pinned signing key, without silently
replacing its immutable native membership binding. Signed-HTTPS request proof is
an explicitly enabled additional possession mechanism for known node signing
keys; it does not claim the client owns a new listening URL. The cloud listener
must have a rooted SignedHttps binding, and outbound client policy must require
that binding for its target. Native response proofs retain their native identity
if a future native listener implements v2; such listener support is not assumed
here. This avoids forcing every native node to rotate identity merely to call
the cloud. The distinction must be reflected in typed authentication checks and
tested, not by mapping a signature onto a TLS PeerKey.

Rollout order:

1. Upgrade all intended evidence-relay participants to dual HMC decoders,
   trusted-key lookup, explicit signed-HTTPS proof and registration/schema support;
   continue existing native mTLS/iroh traffic and HMC v1 emission meanwhile.
2. Distribute/verify original pinned root and evidence chain out of band as
   needed, then explicitly admit the new cloud Node ID with HMC v2 SignedHttps.
   Provision its secret/identity before enabling public routes.
3. Enable v2 HTTPS adapter only for explicitly configured v2 targets. Perform
   membership exchange first, then current-state data checks. No redirect,
   certificate impersonation or fallback to unauthenticated HTTP/mTLS guessing.

Old nodes reject an HMC v2 page atomically and cannot advance its membership
cursor. Existing synchronization must then stop rather than skip that page and
continue data with stale membership. This intentionally prevents a mixed fleet
from remaining transparently functional after v2 evidence enters its log.
There is no v1 translation or selective filtering (which would corrupt evidence
and cursor semantics). An old node may still locally operate using old trust;
compatibility is fail-closed at exchange, not magical remote revocation of old
software. If old-node ongoing participation is required, a separately designed
compatibility protocol is needed; it is not provided by this draft.

The encapsulated HPQ/HPS codec remains v1; upgrading its nested Certificate
reader to accept HMC v2 is required. Peer Status currently advertises record
limits, not HMC capabilities: do not treat successful Status as HMC v2 support.
Use the explicit registration/profile decision until a separate capability
contract exists. Unsupported v2 stops; never automatically downgrade security.

## 10. Suggested implementation boundaries (status as of 2026-11-21)

Implemented locally with tests: protocol codec and fixtures, identity pins with
provenance/migration, the typed signer and verifier, native `ChallengeStore` over
SQLite, the native Peer v2 entry and URL transport, the `harmion-worker` KV/host
boundary, the peer-only DO node `Store` over a synchronous SQL host, host-only
provisioning, a read-only Worker admin API, the wasm-bindgen DO bridge over
`transactionSync`/`sql.exec`, and the Worker fetch/assets/routing wiring (all
exercised in local workerd). Not implemented: real Cloudflare deployment, Access
policy, secret distribution, TLS termination, and DO startup history verification.

- `protocol::https_peer`: canonical typed frame codecs, exact signing inputs,
  hashes and fixtures; no sockets/secret storage.
- Node/host trusted-identity registry: durable immutable pins with validated
  provenance, safe migration, and current authorization checks; reusable by native
  and DO. Pin updates accompany valid membership receipt, not raw-history scans.
- Typed local signer service: only the fixed four request/response domains plus
  challenge-error, using provisioned existing node key; no generic remote signer.
- Host `ChallengeStore`: durable `issue_or_get`, atomic `consume`, bounded GC,
  quotas, generation and auth-time state. It is local transport state, not a
  replicated log or part of membership.handle's data transaction.
- Worker adapter: allowlisted routing, body/deadline limits, Web-facing policy;
  DO repeats identity checks and owns durable replay state and node Store.
- Native URL `PeerTransport` adapter: WebPKI/no redirects, challenge verification
  before upload, signature/fresh-attempt verification before returning PeerReply;
  bounded retries and existing cursor reconciliation.

A validated exchange token may be an internal nonconstructible type handed to
one handler invocation. It cannot be caller-supplied JSON, persisted as a reusable
capability or used to bypass the handler's current-state checks. No production
source changes are part of this draft.

## 11. Attack and conformance checklist

Before exposing the endpoint, test all of the following with native SQLite and
real DO storage/transaction behavior, not only a JS SQL probe:

- Byte fixtures from an independent encoder for every kind/domain, HMC v2
  SignedHttps, signed errors, maximum lengths and rejection of trailing bytes,
  unknown versions/tags, wrong endian/length, noncanonical origin and inner IDs.
- Strict signature validation; bit flips in method, both paths, cluster,
  sender/audience, origin, body digest/length/class, status and payload.
  Cross-protocol/HMC/reflection and same-key/different-node confusion fail.
- Fake TLS server, valid WebPKI wrong Harmion key, wrong expected audience,
  valid signature wrong origin, 301/302/307/308, SSRF endpoints, proxy Host spoofing
  and duplicate headers never leak payloads/Access secrets or create success.
- No record/certificate upload before trusted challenge verification; no cloud
  data/membership page release or acceptance before exchange proof; metadata-only
  challenge allocates nothing for unknown or bad requester signatures.
- Retired/conflict pinned key can exchange evidence but cannot perform data;
  target-only retirement, unrooted self-signature and recycled key cannot
  authenticate. A retired issuer's cut-excluded new Admit naming a pinned victim
  with another key cannot overwrite/poison its pin or block its evidence access.
  Effective conflicting admissions still fail normal identity validation. Old-DB
  missing pins require explicit safe migration/owner confirmation, not raw history.
- Retirement/local conflict introduced between challenge and exchange, between
  challenge consumption and handler, and while pulling a response blocks data
  in the node transaction. Evidence exchange still converges through conflicts.
- Replay identical exchange concurrently, after consumption, cold start, TTL,
  lazy GC and eviction: at most one dispatch, no stale marker resurrection.
  Replay old challenge request: bounded rows/rate, no renewal, newly issued
  challenge requires new nonce-bound exchange proof.
- Replay a valid old ACK/Cursor/Status/error into a new request, switch responses
  between concurrent attempts, reuse nonce, or restart client mid-attempt:
  never advance confirmation/last-success without the exact live binding.
- Crash before/after issue commit, consume commit, handler begin, page prefix,
  node commit, response sign and write: page/cursor atomicity and fresh-challenge
  retry recover; no false negative/positive acceptance claim.
- Lost ACK followed by fresh signed Cursor, duplicate page retry, current cursor
  beyond fixed high, epoch mismatch, record-ID mismatch and rejected membership
  page preserve existing rollback and reconciliation semantics.
- Evict/restart the DO between challenge and exchange: a live durable challenge
  still works, and a consumed challenge never works twice. Ordinary activation
  never resets replay generation. Explicit recovery generation changes fence
  old attempts. Clock reversal/forward jump, suspension, corrupt high-water and
  full replay table fail safely; deadlines never change update causality. No
  handler runs on consume-store error. Multi-page backlog/retry tests with four
  peers demonstrate bounded progress under the chosen command-sized rate budget.
- Slow/oversized/chunked body, decompression attempt, overlarge header, exhausted
  per-peer/global quota, replay flood and unavailable storage remain bounded;
  recovery evidence cannot starve without an explicit operational alert/policy.
- HMC v1 fixtures/IDs remain identical; old reader rejects HMC v2 without partial
  cursor progress or data fallback; new reader gossips mixed history unchanged.
  Retire an old cloud node, then admit a replacement with a new Node ID/key:
  SignedHttps bindings remain distinct; reject binding/signing_key mismatch and
  cross-node key reuse, not all peers sharing the SignedHttps transport type.
- Concurrent deployment initialization, unknown public routing and restore from
  old snapshot cannot replace identity/root or silently reopen replay state.
- Redaction tests inspect client/edge/DO/tracing errors, secrets and payloads;
  Peer identity cannot invoke future management App API or grant itself authority.

## 12. Decisions still required before implementation is production-ready

Status 2026-11-21: item 4 is implemented for new and old databases (see R33);
items 1, 3, 5–8 remain open, and item 2 is only partly answered by local SQLite
and memory-host tests. Nothing below is frozen by the local implementation.

1. Freeze binary layouts/domain strings, origin parser policy, media type and
   HMC/registration/schema versions with cross-language fixtures; the values
   above are concrete proposals, not assigned production numbers.
2. Prove the chosen DO host supports atomic durable challenge CAS across ordinary
   activation/eviction plus existing synchronous node transactions, including
   exception, Rust Err and Wasm trap behavior. Specify monotonic/wall-clock
   guarantees and the clock-anomaly fail-closed/recovery procedure. Generation
   invalidation is explicit identity/replay-store recovery, not normal cold start.
3. Select a constrained signer API and audited seed provisioning/rotation/backup
   procedure. Key rotation is not supported by reusing the same Node ID today;
   define owner recovery/new identity before deploying or restoring backups.
4. Specify durable known-signing-identity registry schema/provenance and old-DB
   migration against all membership/recovery fixtures. Pin only explicit owner
   registrations or effective admissions at acceptance; preserve old pins without
   letting cut-excluded historical claims poison them. Resolve missing retired/
   contested old-DB pins explicitly; no unsafe all-rooted reconstruction. Keep
   registry authentication separate from mandatory current data authorization.
   **Implemented** in schema v11 with explicit "unknown stays unknown" semantics.
5. Tune TTL, body deadlines, row byte overhead, rate windows and concurrency on
   actual runtime limits/free-tier budgets. Decide evidence fairness/reservations
   and alerts; no free operation or flood-proof claim follows from this contract.
6. Confirm fleet-wide upgrade/old-node exclusion procedure before first HMC v2
   admission; capability discovery and multi-binding/key rotation remain separate.
7. Specify owner-approved endpoint update/egress configuration and optional
   Cloudflare Access service-token delivery/redaction. Route changes are not
   automatic signed membership identity changes.
8. Freeze client outcome reporting: distinguish committed ACK, verified fresh
   cursor observation, uncertain transport failure, and authorization denial;
   never mark an interrupted Android attempt successful merely on reconnect.

External Cloudflare/Access background sources are already recorded in the
[Cloudflare design](../docs/core/cloudflare.md#tailnetと入口の追加保護).
No new external-source verification or deployment validation is claimed here.
