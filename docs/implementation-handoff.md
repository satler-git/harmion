# 実用化改善の引継ぎ — 2026-11-20

## 停止位置と所有者の指示

quota節約のため、**Cloudflareレプリカ本体・Android・Windowsの新規実装には進まず、ここで区切る**。この文書は再開用の記録であり、続きの作業や公開デプロイを自動的に始める指示ではない。

合意した優先順は、復帰同期 → 大容量・長期運用の安定化 → 実際のOSと常設レプリカ。OSは **Linux → Android → Windows**。常設レプリカは **Cloudflare Workers／SQLite-backed Durable Objects** を優先する。Androidは電池・回線制約がある断続接続ノードとし、常時到達可能とは扱わない。Interpreterは後回し。

差分は未コミット。前段の同期状態（R26）の変更も同じ作業ツリーに含まれる。次回は `git status --short` とこの文書を確認し、未コミット変更を破棄・一括上書きしない。

## 2026-11-21 再開分（この文書の続き）

所有者が再開を指示し、下の「次回の実装順」2〜5を実装した。公開デプロイ・Cloudflare実配置・user service有効化は行っていない。diffは引き続き未コミット（`flake.nix`・`flake.lock`のみstaged）。

実装済み（詳細は[実装レビュー](implementation-review.md) R33〜R48）:

- **手順2**: 既知署名identityの永続pin（schema v11）。provenance、退役後の保持、鍵再利用・上書き拒否、旧DB移行（Activeな有効参加者とrootだけをseedし、不明な旧peerはunknownのまま）、退役issuerのcut外Admitによるpoison拒否をテストした。
- **手順3**: HMC v2 `SignedHttps` binding（tag 2、binding鍵==signing_key、v1 bytes不変、Retire/Dismissはv1のまま）、dual reader、登録JSON v4（canonical origin + Ed25519鍵の検証）、独立Python HMC fixture 15件。
- **手順4**: 型付き署名操作と厳格検証、durable challenge store（SQLite schema v12）、同一要求の決定論的再送、CAS消費、TTL・generation・clock逆行fail closed・行数上限・token bucket、退役後evidence許可／data拒否。
- **手順5**: nativeのPeer v2入口（`/peer/v2/challenge`・`/peer/v2/exchange`）とWebPKI/no redirectのoutbound transport。実TLS loopbackでchallenge→exchange→HPSと拒否系を検証。`harmion-worker` crateでDOのhost境界（同期transaction・sync・KV/SQL）とHTTP非依存のPeer入口、peer-onlyのnode Store（record/cursor/membership/pin）を追加し、`wasm32-unknown-unknown` checkを通過。

現行の状態（実装済みと未検証を含む）:

- **Workers管理画面とprovisioning（R39〜R48）**: 登録コード、fetchベースのoutbound同期、DO alarmスケジューラを含む。host専用で冪等なprovisioning、読み取り専用のWorker管理API（capabilities/overview/members/peers/provision-status）、既存Svelte管理画面のcapability対応（WorkerではBearer入力・読み取り専用）、wasm-bindgenのDO bridge（`transactionSync`＋`sql.exec`）、WorkerのPeer/管理API/provisioning/静的アセット配線を実装した。ローカルworkerdでprovision→署名付き同期→Accept/Page→再起動保持まで検証済み。
- 実Cloudflare配置・Accessポリシー・secret配布・TLS終端、DO起動時の全履歴検証、管理APIのrate limitは未実装。bootstrap tokenはprovision後に無効化される（R44）。
- nativeの同期ループはsigned-HTTPS TLSで検証済み（R45）。TLSフォワーダ越しにworkerd DOへ接続するnative↔WorkersのE2Eも確認した（R46）。daemonバイナリを設定から起動するE2Eは未実施。
- 実DO isolateのeviction中のCAS、実CloudflareのTLS・routing・quota・料金・multi-region耐久性、commit途中のクラッシュは未検証。App/Query/OAuth面はpeer-onlyとしてfail closed。
- 実ネットワークでのredirect・SSRF・プロキシ・WebPKI運用。loopbackのテストCA検証は実WebPKIの運用保証ではない。
- native daemonへのPeer v2 listener配線は、所有者提供のWebPKI証明書・origin設定の手順が未確定のため行っていない。
- Android・Windows、Interpreter。

2026-11-22: ここまでの作業を commit `174bc2f` に記録した（未コミット差分なし）。直ちにデプロイする予定はないため、次はInterpreterのローカル実装に進む。

次の実装順:

1. Interpreter（契約確定 → registry/provider_session/invocation → App/Provider API → SDK/CLI → 管理UI）
2. native daemonを設定から起動するE2E（App/管理API・管理画面の初期化を含む）
3. 所有者が必要とした時点でWorkersの本番運用準備（Accessポリシー、secret配布、TLS終端、isolate単位のschemaキャッシュ）
4. Android → WindowsのOSホスト

### Interpreterを始めるための準備

仕様は[specification.md](specification.md) §7と[app-api.md](app-api.md)、モジュール境界は[architecture.md](architecture.md)にある。実装前に次を確定する。

- 登録情報の契約: 名前、契約バージョン、入出力スキーマ、デフォルトQuery宣言の形式
- JSON-RPC 2.0のメソッド名とエラーコード、callのタイムアウト・キャンセル・部分応答の扱い
- Provide/Call権限のgrant表現と、同名提供元の承認・選択（ノード内で一意）
- Provider接続の認証（loopback WS＋アプリ資格情報）と、未接続・再起動・提供元交代時の挙動
- 入力スナップショットのサイズ上限と、displayの扱い

初期スライスは、registryとprovider_session、1つの`call`（入力スナップショット、タイムアウト、キャンセル、構造化data/display/error）、偽提供元を使ったnodeテスト、App APIのlist/describe/call/cancelまでとする。管理UIとSDKはその後。

## 1. native本体で実装済み

| 項目 | 内容 | 詳細 |
|---|---|---|
| 同期状態 R26 | Peerの過去の受領確認、最新の失敗、更新IDごとの照会を永続化。API・SDK・CLIを接続 | [API契約](app-api.md#所有者向け同期状態更新の受領確認) |
| 復帰同期 R27 | Linuxのboottime/monotonic差による復帰推定、runtime再開、ネットワーク状態変化、iroh relay接続を同期ヒントに追加 | [監視](../crates/net-native/src/recovery.rs) |
| 過密な再試行の防止 | 2秒の整定、30秒単位の集約、失敗回数の保持、inflight中の次回要求保持、4枠・待機順を維持。手動モードでは監視も起動しない | [daemon](../apps/daemon/src/main.rs) |
| 大容量取得 R28 | get/conflicts/inspectを認可後・token発行前に借用snapshotからサイズ検査。最終JSONも書込み予算で制限 | [node](../crates/node/src/application.rs)・[serializer](../crates/protocol/src/app_view.rs) |
| 失敗した取得のDB増加防止 | 上限超過の取得でedit token/basisを増やさない。候補・データを省略して成功にせず413。保存receiptと再送結果は従来どおり | [回帰テスト](../crates/store-sqlite/tests/large_reads.rs) |
| Linux運用 R29 | 任意のsystemdユーザーサービスtemplate、明示初期化・更新・停止バックアップ・新identity復元の手順 | [運用手順](linux-service.md)・[unit](../deploy/systemd/harmion@.service) |
| 起動メモリ R31 | schema v7以降の履歴検証をContent別のrow streamへ変更。正本の検証を省かず一つのContent Graphだけを保持 | [verifier](../crates/store-sqlite/src/startup.rs)・[回帰テスト](../crates/store-sqlite/tests/startup.rs) |

起動検証では、正本ログの索引欠落・孤立・誤割当、全体のID／位置重複、canonical bytes・digest・Content・依存関係を検査する。派生索引を根拠に壊れた正本を隠したり、勝手に修復したりしない。schema v6以前の移行は全Graphが必要な従来経路を残した。

### 残っているnativeの制限

- 全履歴は保持する。単一Contentの長い履歴と参加証明の再構築は依然として大きくなり得る。総RAMや起動時間に固定上限を保証したわけではない。
- 一括取得を拒否せずページ／streamで読む契約、巨大な単一項目を取得・解決する契約は未実装。
- 正常取得で発行したtokenと保存request receiptの保持期限・GCは変更していない。未合意のTTLや削除を入れない。
- 相手の更新通知、全OSの復帰検知、同じネットワーク構成のまま起きる上流障害の完全な検知は未実装。周期同期は残す。
- sync-statusの未確認は未受領の証明ではなく、過去の確認は現在オンライン・第三ノード伝播・将来のバックアップ保証ではない。

## 2. 検証済みの範囲

native改善後に以下が通過している。transport codecの追加分の最終結果は下の「停止時の最終確認」に分けて記録する。

- 全Rustテスト、Clippy `-D warnings`、fmt、workspace build。
- HTTPS／irohの独立2・5プロセス同期、中継・分断・競合・再起動・有限の再送。
- 新しい復帰同期の実プロセス検証：SIGSTOP/SIGCONT後に長いbackoffを待たず取得できること、手動モードが自発同期しないこと。**kernel suspendの試験ではない**。
- 実daemonでのバックアップ復元、新Node IDでの再参加と新しい更新の同期。
- Python意味モデル492順列、既存record fixture。
- systemd unitのallowlistと、一時実行ファイル／targetでの `systemd-analyze --user verify`。実サービスの登録・起動はしていない。
- domain・protocol・nodeの `wasm32-unknown-unknown` 向けcheck。
- 復帰同期、大容量取得、起動streamingは独立の読取りレビューでも確認した。

実kernel suspend、ホストの実ネットワーク切替、systemd配下の実lifecycle・ログアウト、公開NAT／リレーの長期障害は未検証。Cloudflareへの配置、アカウント設定、課金操作、CA登録、linger変更も行っていない。

## 3. Cloudflareに向けた技術検証（本体ではない）

[Worker SQL probe](../design_checks/worker_sql/README.md) はloopback・空きport・一時保存領域だけを使う。Harmionの本番ノード・公開認証・DO adapterではない。workerdとRust依存をそれぞれlockし、生成物とnode_modulesは追跡しない。

実ランタイムで確認したこと:

- transactionSyncで例外を投げると正常prefix・cursorをrollbackする。**Errorを普通にreturnするだけだとcommitする**。
- JS Numberでは `9007199254740993` が丸まる。文字列cast／BLOBでi64両端を正確に往復できる。
- TERM・SIGKILL後も保存済みのidentity・records・cursorを保持する。ただしkillはflush後であり、commit途中のクラッシュや電源断の試験ではない。
- 実Rust/Wasmの非`'static` FnOnceを渡し、通常のRust Errで元のエラー・cleanupを保ってSQLをrollbackする。
- JS shimは通常のエラーだけをtagged resultとして戻し、RuntimeErrorをRustのcatch importへ戻さず外へ伝播する。同じgeneric wrapperでpanic後のRust継続カウンタが進まず、poison後のgenerated function呼出しを拒否する。
- panic対象のJsValue ABIと生成JS形を検査し、finally/freeによるpoisoned Wasmへの再突入を避ける。新しいプロセスでの再開は確認した。

**未確認**: 実DO isolateの自動廃棄・再生成、一般のunwind／OOM／全host import、全Transaction port、Harmionの更新検証を含む本番adapter、commit失敗、Cloudflare実配置の耐久性・料金・可用性。技術probeの成功を本体完成とは呼ばない。

## 4. Cloudflareで合意したこと・まだ作っていないこと

- 初版は基本的に同期専用。参加・退役・却下証明の同期も必要。将来は遠隔管理を追加できる構造を保つが、Peer資格を管理grantへ昇格させない。
- HTTPSに加えノード鍵の署名で相互に本人確認する方向、所有者が初期鍵・クラスタ情報を明示的に渡す方向は確認済み。
- 未設定DOを公開要求で初期化しない。nativeのadmin.secretをコピーしない。
- Tailnet限定公開の質問には、通常のWorkersをTailnet端末として待ち受ける構成ではなく、Access等による入口保護や常設ゲートウェイ案とは区別すると回答した。**Tailnet必須という追加仕様は確定していない**。

[設計方針](cloudflare-design.md) と [署名付きHTTPSのdraft](../contracts/https-peer-v2-draft.md) を読むこと。draftの具体的なwire／quotaは実装とfixtureで確認する提案であり、公開APIの運用保証ではない。

設計の独立レビューで修正した重要点:

1. 通常のDO cold startで未使用challengeを無効化しない。durable single-use状態を継続する。
2. rawなhistorically rooted Admitを署名鍵の権威にしない。退役issuerのcut外の主張で他ノードの認証・回復経路をpoisonできない、provenance付きの既知identity pinが必要。
3. SignedHttpsのbindingは鍵を明示的に区別する。unit variantの等値で退役cloudと復元先を同一binding扱いしない。
4. quotaはquiet roundだけで枯渇させず、制御command・ページ・再試行を含めて設計する。
5. nonce消費のdurable完了を待ってから既存node handlerへdispatchし、その結果もflush／output gate後にのみACKとして公開する。

### 区切りとして完了したtransport codec

[実験的codec](../crates/protocol/src/https_peer.rs) と [7件のfocused tests](../crates/protocol/tests/https_peer.rs)、[独立Python fixture生成・照合](../design_checks/https_peer_fixture.py)を追加した。5種類のframeとresponse-errorについて、合計18ファイルのbytes・署名入力・hash fixtureを保持する。

boundedなcanonical encoding、型付きのmessage/error、用途別の署名入力、frame hash、標準URL parserでのcanonical HTTPS DNS origin検査、inner HPQ/HPSのidentity/class整合を扱う。既存HPQ/HPS・record・HMC v1の元bytesを保持する。

**署名の暗号検証、信頼registry、freshness判定、challenge永続化、HMC v2、HTTP入口、Cloudflareレプリカは実装していない。** signatureはcodec上は不透明な64 bytesとして扱い、decode成功を認証成功とは呼ばない。nativeのcapabilitiesや公開endpointも追加していない。

### 次回の実装順（2026-11-21に1〜5を実行済み。残りは上の再開分を参照）

1. 未コミット差分と停止時の検証結果を確認する。
2. 既知署名identity pinのprovenance／bootstrap／旧DB移行を確定し、退役issuerによるpoison拒否をテストする。
3. 必要なHMC v2 SignedHttps binding・登録形式・旧v1 bytes保持・dual reader互換性を実装する。旧ノードの失敗をskip/fallbackで隠さない。
4. bounded durable challenge storeと署名・freshness検証を実装する。cold start、再送、古いACK、clock異常、権限の再検査をテストする。
5. Rust/DO保存adapterとPeer入口、native HTTPS URL transportを接続する。WebPKI・no redirect・origin allowlist・秘密のredactionを維持する。
6. ローカルでHarmion本体のend-to-end同期・中断・退役・cold startを検証する。実デプロイはその後の所有者確認を要する。
7. AndroidのOSホストと断続的な同期、次にWindowsの所有者保護と常駐へ進む。

## 5. 再現環境とコマンド

`flake.nix` でnixpkgsとfenixをpinし、`nix develop` がrustc 1.100（wasm32 std込み）・clippy・rustfmt・node/npm・python3・C toolchainを用意する。`flake.nix`・`flake.lock`はgitの追跡対象に含まれている必要がある。`sccache`がこのホストで失敗したため、検証時は `RUSTC_WRAPPER=` を指定した。ソースの仕様を変える回避策ではない。

```sh
# nativeの通常検証
nix develop --command bash -c '
  set -e
  export RUSTC_WRAPPER=
  cargo test --workspace --offline
  cargo clippy --workspace --all-targets --offline -- -D warnings
  cargo fmt --all -- --check
  cargo build --workspace --offline
  python3 design_checks/recovery_process.py
  python3 design_checks/recovery_process.py --iroh
  python3 design_checks/linux_service.py
'

# 共有コアのWasm check
nix develop --command bash -c '
  export RUSTC_WRAPPER=
  cargo check -p harmion-domain -p harmion-protocol -p harmion-node \
    --target wasm32-unknown-unknown --offline
'
```

irohの実プロセス検証には事前に `cargo build -p harmion-net-native --example local_relay` が必要。Workersのnpm ci・standalone Rust build・wasm-bindgen CLIの一致確認・probeの再現コマンドは [probe guide](../design_checks/worker_sql/README.md) にまとめた。公開デプロイやuser serviceの有効化を検証スクリプトへ足さない。

## 停止時の最終確認

- transport codecの追加後も、全workspace Rustテスト、Clippy `-D warnings`、fmt、共有コアのWasm checkが通過した。
- 独立PythonのHTTPS envelope fixture 18件と、Linux serviceの静的検証が通過した。
- nativeの実プロセス検証・SQL／Rust-Wasm probeの結果は上記のとおり。それぞれの未検証範囲も残した。
- 作業に使ったバックグラウンドジョブは完了結果を回収済み。ローカルworkerd等の試験サーバーも停止済み。サブエージェントの追加作業は終了した。
- Cloudflareレプリカ本体・Android・Windowsへは進んでいない。公開デプロイ・ホストのサービス有効化も行っていない。
- **ここで作業を終了する。続きは所有者が再開を依頼したときに、この文書から始める。**
