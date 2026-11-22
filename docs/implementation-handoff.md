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
- Android・Windows。Interpreterのnative v1初期スライスは2026-11-22に実装した（R52）。

2026-11-22: ここまでの作業を commit `174bc2f` に記録した（未コミット差分なし）。直ちにデプロイする予定はないため、次はInterpreterのローカル実装に進む。

### 2026-11-22 追記（Interpreter前の穴埋め）

所有者との合意に基づき、Interpreterの前に残っていた穴を塞いだ。詳細は[R49〜R51](implementation-review.md)。この節の作業は commit `b82adfd` に記録した。作業ツリーはクリーンで、公開デプロイは行っていない。

- **9（Worker参加の配線）**: `harmion_node::provisioning::export_manifest`、App API `provision-manifest`、CLI `harmion provision-manifest`を追加した。承認済みのSignedHttps Peerについて、root anchor・root-firstのAdmission列・Activeなcontactsを出力する。実`harmiond`2台とworkerd DOをTLS越しにつなぐ[E2E](../design_checks/worker_daemon.py)で、provision→push→参加→Workerからのpull→再起動→再provision拒否を確認した。
- **10b（保持）**: schema v13で編集トークンとrequest receiptに作成時刻を付け、30日より古い行をGCする。回収済みトークンはHTTP 410 `token_expired`。方針前の行は削除しない。daemonが起動時と6時間ごとに回収する。
- **8（nativeのv2入口）**: nativeはiroh/HTTPSのまま。WorkerのoutboundはSignedHttps以外をスキップするため現行の端末起点同期は成立する。`peer_v2::serve`は未配線で、nativeへv4 SignedHttps登録を使うと同期だけ失敗する点を運用上の注意として明記した。公開Linuxピアが必要になった時点で実装する。
- **10a（大容量）**: 保存要求1 MiB・応答8 MiBの上限を明記し、単一Contentのstream/page取得はv1で提供しない。
- **11（鍵変更・遠隔管理）**: v1対象外として明記。端末交換は新Node IDでの復元を使う。
- **未実施のまま**: 実CloudflareのAccess/TLS/secret/課金、workerd自体の再起動を含むE2E、Android/Windows。Interpreterのnative v1初期スライスはR52で実装した。

### 2026-11-22 追記（統合OAuth接続とInterpreter権限）

所有者の指示により、Interpreterの未実装範囲とOAuth権限付与を統合した。詳細は[R53](implementation-review.md)。差分はこの時点で未コミット。

- 初回接続を`connect-start` → 管理UI「接続」承認 → `connect-status`/`connect-complete`に統合した。Provider登録・OAuthクライアント作成・grant確定は承認時の同一transactionで行う。
- OAuth scopeに`call`と`provide:BASE64URL`/`provide:all`を追加し、`provide_names`で事前許可されたProvider名は追加承認なしでapprovedになる。grantから外れた名前の登録はsupersededになる。`call`/`provide`はRead/Writeを含意しない。
- schema v15（`app_provide_names`）とv16（`connection_requests`）を追加した。接続要求は5分・32件、完了はPKCE＋単回secret。応答を失った場合は要求を作り直す。
- ブラウザProvider用に30秒・単回ticketとOAuth originの`/provider/connect?ticket=...`を追加した。Webアプリ用に`/connect`ヘルパーとloopback CORSを追加し、承認を`window.opener.postMessage`で通知する。
- Rust SDK・CLI・管理UI・`connection.json`の`oauth_origin`を更新した。管理UIには「接続」画面を追加した。
- 所有者認証は今回も`admin.secret`＋管理UIセッションのまま。クラスタ所有者・Owner Key・Passkey・OS鍵保管は[メモ](owner-authentication-memo.md)に留め、実装していない。

### 2026-11-22 追記（R54）

統合接続・OAuth決定経路でもgrant変更後のセッション再評価を行うようにし、`provide_names`の手動上限、OAuth originのticket再発行（`/connect/ticket`）を追加した。詳細は[R54](implementation-review.md)。`connect-complete`の単回性とticketのメモリ保持は残る制約。

### 2026-11-22 追記（R55）

実`harmiond`＋`harmion-admin`の統合接続E2E、Provider WebSocket Ping、`ProviderClient`の再接続/backoff、`connect-complete`の60秒再送キャッシュ、Providerごと8件の同時上限を追加した。詳細は[R55](implementation-review.md)。Pong期限監視・永続audit・invocation自動再送は残る制約。

次の実装順:

1. Interpreter（契約確定 → registry/provider_session/invocation → App/Provider API → SDK/CLI → 管理UI） — native v1初期スライスはR52、統合接続と権限はR53で完了
1b. 所有者認証（R2端末承認＋ノード検証SSH assertion＋監査はR56で実装済み。[設計案](owner-authentication-design.md)。残りはWebAuthn/OS鍵保管/Cluster Owner/threshold） — 追加判断待ち
2. 所有者が必要とした時点でWorkersの本番運用準備（Accessポリシー、secret配布、TLS終端、isolate単位のschemaキャッシュ）
3. Android → WindowsのOSホスト
4. E2Eの残り（workerd再起動を含むDO耐久、管理画面を含むdaemon起動の通し確認）

### Interpreterを始めるための準備

仕様は[specification.md](specification.md) §7と[app-api.md](app-api.md)、モジュール境界は[architecture.md](architecture.md)にある。実装前に次を確定する。

- 登録情報の契約: 名前、契約バージョン、入出力スキーマ、デフォルトQuery宣言の形式
- JSON-RPC 2.0のメソッド名とエラーコード、callのタイムアウト・キャンセル・部分応答の扱い
- Provide/Call権限のgrant表現と、同名提供元の承認・選択（ノード内で一意）
- Provider接続の認証（loopback WS＋アプリ資格情報）と、未接続・再起動・提供元交代時の挙動
- 入力スナップショットのサイズ上限と、displayの扱い

初期スライスは、registryとprovider_session、1つの`call`（入力スナップショット、タイムアウト、キャンセル、構造化data/display/error）、偽提供元を使ったnodeテスト、App APIのlist/describe/call/cancelまでとする。管理UIとSDKはその後。


### 2026-11-22 Interpreter v1初期スライス（R52）

引継ぎの「Interpreterを始めるための準備」に列挙した契約を[interpreter-v1.md](../contracts/interpreter-v1.md)に確定し、初期スライスを実装した。詳細と検証は[R52](implementation-review.md)に記録した。この節の変更は未コミットで、公開デプロイは行っていない。

実装した範囲:

- **契約**: 登録JSON、`provide`/`call` grant、`manage_apps`による承認、承認済み名の一意性と`replace`、App APIのlist/describe/register/approve/call/cancel、Provider WebSocketのJSON-RPC（attach/invoke/cancel）、timeout・cancel・サイズ上限・エラー。
- **protocol**: `protocol::interpreter`の型・検証と、限定RFC 6455 framing（`protocol::websocket`）。
- **node**: `interpreter::registry`（SQLite schema v14）、`provider_session`、`invocation`（編集トークンが固定したSnapshotの生成、Read検査、同時実行状態）。
- **net-native**: `/provider/v1/connect`の認証付きWebSocket、attach後のセッション管理、invoke/cancel、timeout・切断・置換・grant失効時の終了、App APIへの接続。
- **SDK/CLI/UI**: Rust App SDKの`interpreter_*`、Provider SDKの`ProviderClient`、CLI 6操作、管理UIの一覧・定義表示・承認、grant編集の`call`/`provide`。
- **テスト**: protocol wire、SQLite registry/承認/名前競合/スナップショット固定/Read/再起動/失効、状態機械、実TLSのProvider接続（拒否・二重attach・所有元不一致・成功・Providerエラー・timeout+cancel・明示cancel・切断・grant失効）、Provider SDKのattach/invoke/timeout cancel。

残る範囲（v1の欠陥ではなく対象外または未検証）:

- Provider SDKは単一接続を扱うが、自動再接続・常駐・バックオフのループはアプリ側。idle heartbeatは送らない。
- 管理UIは一覧・定義表示・承認のみで、callの入力組み立て・実行画面はない。callはSDK/CLIから行う。
- Worker/DOはInterpreterを提供せず、App APIはfail closed。遠隔Interpreter・自動起動・サンドボックス・ストリーミング・入力Contentのページ取得・request receipt・Provider単位のgrant/quota/監査はv1対象外。
- Android/WindowsのProvider常駐・実行、実Cloudflare、実daemonを設定から起動する通しE2Eは未検証。

次にInterpreterで進めるなら、Provider SDKの再接続ループ、実アプリでのcall統合、必要になった時点の名前単位grant・監査、Androidの実行方式の順が自然である。Workersの本番運用準備を優先する場合は、R39〜R52の未実施範囲を先に扱う。

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
