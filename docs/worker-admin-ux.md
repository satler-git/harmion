# Worker 管理画面の情報設計

2026-11-27 / 状態: P0 + P1 実装済み（native は共通修正のみ）

対象は同じ Svelte UI の 2 つの配信形態。

- **native**: ローカルの `harmion-admin`。全画面・書き込みあり。
- **worker**: Cloudflare Workers が配信する読み取り専用の `harmion-admin`。Peer 専用レプリカ。

実デプロイの画面は Cloudflare Access の内側にあり、ブラウザセッションなしでは確認できない。以下の現状はリポジトリのコードと API 契約、ローカル workerd probe の構成に基づく。

## 1. 現状の確認

Worker mode では `capabilities.sections` により **概要** と **Peer** だけがナビに残る。書き込み系の画面は描画されない。`/capabilities`、`/overview`、`/members`、`/peers`、`/provision-status`、`/registration-code` が実装済みだが、UI が実際に使うのは `/capabilities` と `/overview` だけ。

| 情報 | API にあるか | Worker UI の表示 | 問題 |
|---|---|---|---|
| 設定不備・未初期化 | `/ready` が不足名を返す | 出ない | 画面が正常に見えてしまう |
| `provisioned_state` と env の一致 | `/ready` が判定 | 出ない | env 変更後の再登録漏れに気づけない |
| 初期設定日時・受付状態 | KV に `provisioned_at`、env に `ALLOW_PROVISION` | 「設定済み/未設定」だけ | いつ・何が設定済みか分からない |
| 自分の参加状態 | `members.nodes` に含まれる | 一覧の中に混ざる | 未承認・退役・競合が目立たない |
| 署名公開鍵 | `overview.identity.signing_key` | **出ない** | UI が `binding.signing_key` を参照しており、実際は top-level（[Overview.svelte:129](../apps/admin/ui/src/pages/Overview.svelte#L129)、[admin.rs:224](../crates/worker/src/admin.rs#L224)） |
| Cluster Root / Epoch | `cluster_root` / `identity` | 出ない | 監査時に不足 |
| メンバーの binding・公開鍵 | `members` は node/state のみ | 出ない | 誰がどの入口か分からない |
| Peer の pin 状態 | `peers` にある | 出る | 良い |
| 保留中の退役証明 | `members.pending_retirements` にある | **出ない**（readOnly 分岐で未描画） | native で処理が必要なのに見えない |
| 登録コード | `/registration-code` にある | 出ない | Worker の承認に CLI/curl が要る |
| 最終受信時刻 | 記録していない | 出ない | 「最新か」を答えられない |
| 保持する更新数・容量 | 保存先から集計可能 | 出ない | 「何を持っているか」が分からない |
| Peer ごとの受信位置 | `peer_cursors` にある | 出ない | どこまで届いたか分からない |
| 受信方向・同期間隔 | `SYNC_INTERVAL_MS`、alarm、schedule | 出ない | 「同期できない」理由が分からない |
| 最新の受信更新 | `accept_log` から取得可能 | 出ない | 個別の同期確認ができない |

`members` は `capabilities` で `true` だが `Page` 型に存在せず、独立した画面にはなっていない（[types.ts:1](../apps/admin/ui/src/lib/types.ts#L1)）。現状は Peer 画面に混在している。

## 2. 足りない情報の優先順位

Worker の管理画面は「操作する場所」ではなく、**このレプリカが正常で、最新で、何を保持しているかを確認する場所**として設計する。

### P0: これがないと状態を判断できない

1. **状態**: 設定、初期化、identity 一致、自分の参加状態をまとめた 1 つの結論。
2. **最終受信**: 最後に data-class の signed-HTTPS exchange を受けた時刻。未受信ならその旨。
3. **保持量**: 受理した更新数、Content 数、レコード容量。
4. **署名公開鍵・Epoch・Cluster Root**: 識別情報の監査。
5. **登録コード**: 未承認・identity 不一致のときに native で承認するため。
6. **保留中の退役証明**: 読めないと native 側の対応漏れになる。

### P1: 原因調査に効く

7. **メンバー詳細**: 状態、binding、endpoint、署名公開鍵。
8. **Peer ごとの受信位置**: どこまで受信したか。
9. **同期方式と schedule**: `native から受信`、alarm の次回、失敗回数。
10. **初期設定の受付状態**: bootstrap がまだ有効か、再初期化が許可されているか。

### P2: あると便利だが後回し

11. 最新の受信更新（update ID、Content ID、author、edited_at）。
12. 直近 N 件の受信履歴。
13. native 側の `sync-status` を Overview に出す。

## 3. 画面構成案

ローカル workerd の probe データで撮影した画面: [概要](worker-admin-overview.png) / [Peer](worker-admin-peers.png) / [未設定](worker-admin-unconfigured.png)。

### 概要（worker）

```
[正常] 最終受信 3分前
┌────────────┬────────────┬────────────┬────────────┐
│ 最終受信    │ 保持する更新 │ Content     │ Peer        │
│ 3分前       │ 1,234       │ 321         │ 2 / 2 pin   │
└────────────┴────────────┴────────────┴────────────┘

この Worker
  Node ID / Cluster ID / Epoch / Origin
  署名公開鍵 / Cluster Root
  初期設定: 済み（2026-11-22 10:00）・設定と一致

同期
  受信: native から（最終 12:31）
  保持位置: 1,234
  Peer ごとの受信位置
  外部同期: 無効（native が開始）

承認と初期設定
  未承認なら登録コード + QR
  承認済みなら「承認済み」と詳細の折りたたみ
```

- 状態は文章ではなくバッジと値で示す。理由が必要なときだけ、その行に短いラベルを付ける。
- 「管理モード 読み取り専用」「Worker レプリカ」のような重複したカードは廃止し、実際の状態に置き換える。
- 未設定・identity 不一致・未承認のときだけ、セットアップの詳細を `<details>` で出す。

### Peer（worker）

1. **メンバー**: 自分を先頭に、状態バッジ、binding、公開鍵の短縮表示。Node ID はコピー可能。
2. **Peer 接続**: transport、endpoint、pin の由来、退役。受信位置があれば併記。
3. **保留中の退役証明**: 対象・発行元・証明 ID。確定/却下は native で行う旨を 1 行だけ添える。
4. 空状態は「まだ受信していない」「登録された Peer はない」のように、次の行動が分かる最小限にする。

## 4. API 案

### 4.1 `/admin/host-status`（新規、bearer）

Worker ホストだけが知る情報を返す。Rust の node store とは分離する。

```json
{
  "config": { "ok": true, "missing": [], "invalid": [] },
  "provisioning": {
    "provisioned": true,
    "provisioned_at": "1764...",
    "identity_matches": true,
    "bootstrap_enabled": false,
    "allow_reprovision": false
  },
  "sync": {
    "interval_ms": "0",
    "alarm_next_at": null,
    "last_inbound_at": "1764...",
    "schedule": [
      { "node": "...", "failures": "0", "next": "1764..." }
    ]
  }
}
```

- `missing` / `invalid` は env 名ではなく「クラスタID」「公開URL」のような表示名に寄せる。値は返さない。
- `identity_matches` は fingerprint を返さず boolean だけ返す。
- `last_inbound_at` は `/peer/v2/exchange` が成功したときに KV へ記録する。毎 exchange ではなく、前回から一定時間経過したときだけ書く。メタデータの書き込み失敗で peer 応答を壊さない。
- パスは `/admin/*` 配下に置く。既存の `run_worker_first`、body limit、bearer 検査にそのまま乗る。

### 4.2 `/overview` の追加（worker のみ）

```json
{
  "self_state": "active | unconfirmed | retired | conflict | unconfigured",
  "stats": {
    "contents": "321",
    "accepted_updates": "1234",
    "record_bytes": "456789",
    "membership_high": "12"
  },
  "latest_update": {
    "update_id": "...",
    "content_id": "...",
    "author_node_id": "...",
    "edited_at": "1764..."
  },
  "members": {
    "members": [
      {
        "node_id": "...",
        "state": "active",
        "signing_key": "...",
        "binding": { "transport": "signed_https", "origin": "https://..." }
      }
    ],
    "pending_retirements": [],
    "confirmed_certificates": "5"
  },
  "peers": [
    {
      "node_id": "...",
      "retired": false,
      "pinned": true,
      "pin": { "signing_key": "...", "provenance": "owner_registration" },
      "binding": { "transport": "signed_https", "origin": "https://..." },
      "received_through": "1230",
      "membership_received_through": "12"
    }
  ]
}
```

- 64-bit の値は今まで通り文字列。
- `stats` は Worker の `meta` に持つ増分カウンタから返す。native の `node_metrics` を流用しない（`visible_contents` は peer-only では作っていない）。初回だけテーブルを集計し、以後は受理時に更新する。
- `latest_update.edited_at` は端末が付けた時刻であり、受信時刻ではない。UI では「更新日時（端末）」と区別する。
- `members` の詳細は `Status::Active(member)` のときだけ返す。退役・競合・未確認は node と state のみ。

## 5. 実装方針

1. **Rust**: `admin::overview` に `self_state`、`stats`、メンバー詳細、Peer の受信位置を追加。`stats` は `meta` の増分カウンタ、受信位置は sender ごとの latest cursor を使う。
2. **Worker host**: `worker.js` に `/admin/host-status` と最終受信の記録を追加。`/admin/*` は既に `run_worker_first` と body limit の対象なので、追加のルーティング設定は不要。
3. **UI 型**: `types.ts` に `WorkerHostStatus`、`WorkerStats`、`WorkerMember`、`WorkerPeer` の受信位置を追加。`Page` に `members` は増やさない。
4. **UI 概要**: worker mode のときだけ `WorkerStatus` 相当のブロックを描画。native の metrics 表示は変えない。
5. **UI Peer**: readOnly 分岐をメンバー / Peer / 保留証明に整理。native 分岐は触らない。
6. **登録コード**: `/registration-code` を worker mode で取得。未承認のときだけ QR を表示し、承認済みなら折りたたむ。QR 生成は既存の `qrcode` を使う。
7. **共通修正**: 署名公開鍵を `overview.identity.signing_key` から表示する。`capabilities` の `members: true` は実画面がないので削除するか、Peer 内の節であることを明示する。ログイン画面の「Cloudflare Access の背後でのみ利用できます」は、Worker が Access JWT を検証していないため、管理者トークンの説明に置き換える。

native の画面構成は変えない。共通で直すのは署名公開鍵の表示バグと、capabilities の整合だけにする。

## 6. やらないこと

- Content の閲覧・編集、App/Query/OAuth/Export の公開。Worker は peer-only のまま。
- 承認・退役・同期の書き込み操作。Worker では提供しない。
- Cloudflare の請求・quota・リクエスト数・DO のストレージ容量。Cloudflare の dashboard / analytics が正本。
- Cloudflare Access の検証済み表示。Worker 自身は `Cf-Access-Jwt-Assertion` を検証していないため、「Access の背後」を UI の保証として書かない。
- 環境変数の値、seed、token の表示。

## 7. 決定

1. 範囲は P0 + P1。P2（最新更新・履歴）は次段。
2. ホスト情報は `/admin/host-status` に分離する。
3. peer exchange 成功時に `sync/last_inbound` を KV へ書く。失敗は peer 応答に影響させない。
4. native は画面構成を変えず、署名公開鍵と capabilities の共通修正だけにする。
5. 登録コードは、未承認・不一致のとき概要に表示し、承認済みなら折りたたむ。

## 8. 検証

実施済み:

- `cargo test -p harmion-worker`。
- `cargo check -p harmion-worker --target wasm32-unknown-unknown`（nix dev shell）。
- `cargo check -p harmion-admin`（native の shared 修正）。
- `npm run check` / `npm run build`（`apps/admin/ui`）。
- `design_checks/worker_do/check.py` のローカル workerd（provision → 署名付き同期 → 再起動保持）。
- ローカル workerd の `/admin/host-status` が bearer なし・誤 bearer で 401、ありで 200 を返すこと。`cache-control: no-store`、秘密値の非混入、マイクロ秒の時刻も確認。
- `overview.peers` が配列で返り、`received_through` / `membership_received_through` / `stats` / `self_state` / `identity.epoch` が入ること。
- `/admin/host-status` の応答に seed / token / 環境変数の値が含まれないこと。
- 最終受信が membership-only の exchange では更新されず、data-class の exchange で更新されること（コードと probe で確認）。

未確認:

- 未設定はローカル workerd のブラウザ表示で確認済み。未承認・identity 不一致の見た目はまだ確認していない。
- 実 Cloudflare デプロイ。Access のセッションが必要。

## 9. 実装結果

- `crates/worker/src/admin.rs`: `self_state`、`stats`、メンバーの `signing_key` / `endpoint` / `binding`、Peer の受信位置、`identity.epoch`。`overview.peers` が `{"peers": [...]}` の二重包みになっていた既存バグも修正。
- `crates/worker/src/store.rs`: `replica_stats`（Content 数・受理更新数・レコード容量・membership high）を `meta` の増分カウンタで返す。初回だけ既存ログから backfill。
- `crates/node/src/storage.rs`: `ReplicaStats` と既定 `replica_stats`。
- `deploy/cloudflare/src/worker.js`: `/admin/host-status`、`sync/last_inbound`、schedule の `last_at` / `last_ok`。
- `apps/admin/ui`: Worker の状態バナー、最終受信・保持量、同期パネル、登録コード（QR / コピー）、メンバー・Peer・保留証明の読み取り専用表示。署名公開鍵は top-level から表示。host-status 失敗時は「状態取得不可」とし、登録コードは identity 変更で再取得する。
- `capabilities.sections.members` を削除し、画面に存在しないタブを示さないようにした。
- ログイン画面の Cloudflare Access の記述を外し、管理者トークンとローカルの管理画面での操作に置き換えた。

### レビュー後の修正

- Peer cursor は sender の epoch ではなく latest cursor を使う。
- `stats` は増分カウンタ化し、`/overview` のたびに全ログを走査しない。
- host-status は 503 + `cache-control: no-store`。UI は 404 以外を「状態取得不可」とし、正常扱いしない。
- 最終受信は data-class の exchange だけを記録する。
- host-status の時刻はマイクロ秒に統一。
- Worker の管理 API 呼び出しを `/admin/*` 配下に寄せ、Access の保護範囲に揃える。
- 登録コードは identity/refresh 変更で再取得し、エラー時は再読み込みできる。
- 退役・競合・設定不備では承認 QR を出さない。
- Peer の見出し・コピー・ラベル・ARIA を整理。
