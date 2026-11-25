# Harmion for Android — 設計・実装状態
本ドキュメントは [android-node.md](android-node.md) の要求に対するAndroid固有の設計判断と、
現時点の実装範囲・既存仕様との差分をまとめる。要求文書を置き換えるものではない。
最終更新: 2026-11-25。ノード本体・SDK・所有者承認・M3 Expressive UI・
アプリ認可管理・競合解決・ウィジェット・通知・回復コード再登録まで実装。
x86_64 API 36 エミュレータで動作確認済み（物理端末は未検証、第14節）。
## 1. スコープと方針
- 既存Rust Core（`crates/node`、`crates/protocol`、`crates/store-sqlite`、
  `crates/net-native`）と既存Peerプロトコル・認可モデルをそのまま利用する。
- Android固有の実装は、OS統合（ライフサイクル、Binder、WorkManager）、鍵の保護、
  承認UI、SDKに限定する。
- 1アプリ = 1ノード = 1DB。ノードの初期化・開始・停止は
  `NodeRuntimeController` が一元管理し、Activity・Service・Workerが個別にCoreやDBを
  初期化しない。
- root / ADB / Shizuku を前提にしない。
## 2. 既存コードの調査結果
再利用するもの:
| 領域 | 既存実装 |
|---|---|
| 同期・競合解決の意味論 | `crates/domain`, `crates/node` |
| Peerプロトコル・ページ転送 | `crates/protocol`, `crates/net-native/src/sync.rs` |
| HTTPS / iroh / signed-HTTPS transport | `crates/net-native` |
| SQLite永続化・migration | `crates/store-sqlite` |
| App HTTPS API・OAuth・connect | `crates/net-native/src/app.rs`, `crates/node/src/{application,oauth,connect}.rs` |
| App APIのRustクライアント | `crates/client` |
| 参加・退役・QRペイロード | `contracts/membership-v1.md`, `apps/admin/src/transfer.rs` |
不足していたもの（本実装で追加）:
1. daemonの起動処理が `apps/daemon/src/main.rs` に閉じており、ホスト（Android）から
   再利用できるランタイム境界が無かった。
2. 同期スケジューラがシグナル前提のdaemonループ内にあり、ホストのポリシー
   （Wi-Fi・従量制・バッテリー・ローミング）を差し込めなかった。
3. JNI/FFI境界が無かった。
4. Androidアプリ（Service、承認UI、SDK）が無かった。
5. アプリ一覧APIが無かった → App API `apps` とSDK `HarmionAdminClient.apps()` を追加。
6. 認可期限の変更手段が無かった → `change-grants` に任意の `expires_at` を追加。
残る制約（第9節・第13節）:
- 所有者SSHSIG検証はEd25519/ECDSA P-256を内製化済み（`crates/node/src/sshsig.rs`）。
  RSA/FIDO2はホストの`ssh-keygen`へフォールバックする（第9節）。
- 新規OAuth認可の既定family期限は30日のまま（延長は`change-grants`で可能）。

## 3. 全体構成
```
Third-party app (Capture/Wiki/...)
  └─ Harmion SDK (Kotlin) ── AIDL ──▶ HarmionNodeService (bound service)
        │                                   │
        │ JNI: libharmion_sdk_native.so     │ same process
        │ (crates/client-ffi)               ▼
        │                            NodeRuntimeController
        │                                   │ JNI: libharmion_android.so
        │                                   ▼ (crates/android)
        │                            harmion-runtime
        │                                   ├─ harmion-net-native (App API / Peer)
        │                                   └─ harmion-store-sqlite (node.db)
        └─ pinned HTTPS (127.0.0.1) ──────▶ App API
```
- 管理画面・承認Activityも同じプロセスで `NodeRuntimeController` を使う。
- データプレーンは既存HTTPS App APIのまま。Binder上にQuery/Edit/Subscribeを
  再実装しない。
- プロセス分離（`:node` など）はしていない。DBの単一書き込み主体を保証するため、
  ランタイム所有者は常に1つ（`HarmionApplication` が保持）。
### 追加クレート
| クレート | 役割 |
|---|---|
| `crates/runtime` (`harmion-runtime`) | ノード初期化・起動・停止・状態・同期スケジューラ・ポリシーゲート。daemonとAndroidが共有する |
| `crates/android` (`harmion-android`) | ノード用JNI cdylib。JSON APIとJNIの薄い層 |
| `crates/client-ffi` (`harmion-sdk-native`) | SDK用JNI cdylib。既存Rustクライアントの再利用 |
`crates/runtime` はdaemonと同じ `node.json`（version 1）・同名ファイル・同じDBを使う。
daemon（`harmiond run`）とAndroidアプリは同じ `NodeRuntime::start` / `shutdown` / `failure` を使う。
Android固有設定（同期ポリシー等）はSharedPreferences側に置き、`node.json` を拡張しない。
## 4. ノードのライフサイクル
- `NodeRuntimeController.ensureStarted()` が `harmion_runtime::NodeRuntime::start`
  を呼ぶ。同一プロセス内の開始・停止はMutexで直列化する。
- 稼働理由: 管理画面の表示中、Bound Serviceへの接続中、WorkManager実行中。
  すべて `acquire()` / `release()` の参照カウントで表現し、最後の利用者が消えて
  30秒経過したら停止する（`IDLE_STOP_MILLIS`）。
- プロセスは予告なく終了しうる。正常終了処理に整合性を依存させない。
  停止はSQLiteのWALとトランザクションに依存し、再起動時は保存済みカーソルから再開する。
- 常駐は既定でしない。bound service / Activity / WorkManagerの参照が消えると
  30秒で停止する。ユーザーが設定で「常時稼働」を有効にした場合のみ
  `KeepAliveService`（foregroundServiceType=dataSync）が参照を持ち続け、
  低優先度の継続通知に状態とピア数を表示する。通知権限が無くても常駐自体は動く。
- ロック画面（Direct Boot）中はCEデータを持つアプリのActivityが解決されないため、
  端末の初回アンロックまでアプリを起動できない（Androidの仕様）。ノードの自動起動はしない。
## 5. ストレージ・鍵
- ルート: `filesDir/harmion/node`（アプリ専用領域）。`HOME` には依存しない。
- 既存の `harmion_local::ProtectedDir` をそのまま使う（0700ディレクトリ、0600ファイル、
  symlink拒否、owner一致）。app専用領域でも同じ検査を通す。
- ファイル: `node.db` / `node.json` / `connection.json` / `peer.json` /
  `certificate.der` / `private-key.der` / `admin.secret` / `iroh.secret` /
  `oauth-identity.json`。初期化は `node.json` を最後に書き、途中失敗したディレクトリは
  有効なノードとして開かない。
- Android Auto Backupは無効（`allowBackup=false` + `data_extraction_rules.xml`）。
  ノードID・秘密鍵・DBを複製しない。機種変更は新規ノードとして参加する。
- 鍵の分離:
  - node signing key / iroh secret / transport TLS key / App API TLS key:
    アプリ専用領域のファイル（既存Coreの契約を維持。Coreは32byte seedを要求する）。
  - 利用アプリのトークン: Android KeystoreのAES-GCM鍵で暗号化してSharedPreferencesへ
    （`org.harmion.sdk.TokenStore`）。バックアップ対象外。
  - owner signing key: Android KeystoreのAES-GCM鍵（BIOMETRIC_STRONG必須）で
    seedを暗号化してSharedPreferencesへ。署名は生体認証のたびに実行（第9節）。
## 6. 同期ポリシーとスケジューリング
設定（`SyncSettings`、SharedPreferences `org.harmion.android.sync`）:
| 設定 | 値 | 既定 |
|---|---|---|
| 自動同期 | on/off | on |
| ネットワーク | Wi-Fiのみ / 非従量制 / すべて | 非従量制 |
| バッテリー | 制限なし / 残量低下(15%)で停止 / 充電中のみ | 残量低下で停止 |
| ローミング | 許可 / 禁止 | 禁止 |
| 定期同期 | 15 / 30 / 60分 | 30分 |
| ローカル更新時 | on/off | on |
| アプリ接続時 | on/off | on |
| ネットワーク復帰時 | on/off | on |
- 判定は `PolicyEvaluator` が `ConnectivityManager`（`NET_CAPABILITY_VALIDATED`、
  `NOT_METERED`、`TRANSPORT_WIFI`、`NOT_ROAMING`）と `BatteryManager` で行う。
  Wi-Fi接続と非従量制は区別し、実際に通信可能な経路かを検証する。
- 判定結果は `setSyncPolicy`（FFI）でランタイムの `PolicyGate` に反映する。
  スケジューラは各ラウンド開始前にゲートを確認し、拒否されたら新規ラウンドを開始せず、
  実行中のラウンドは中止する（ページ転送はトランザクションで、永続カーソルから再開できる）。
- WorkManager（`SyncWorker`）は起動契機の供給のみを担う。実行時にもポリシーを再評価し、
  ポリシーを無視するのは「ユーザーが確認した手動同期」だけ（`sync_now`）。
  バックグラウンドは `sync_gated` を使い、ゲートを迂回できない。
- トリガ: 定期、ローカル保存（`SharedStore`の通知）、アプリ接続、ネットワーク復帰、
  手動、Peerの着信（着信はサーバー側で処理され、次のラウンドで結果が反映される）。
- 連続更新は既存のrecoveryヒント（2秒待ち・30秒に1回）とバックオフ（指数+ジッタ、
  最大1時間）で集約する。
- 同期が停止していてもローカル保存・参照は可能。
## 7. 外部アプリとの接続
### Bound Service（制御プレーン）
`org.harmion.android.service.HarmionNodeService`（exported、明示コンポーネントbind）。
AIDL `IHarmionNodeService`:
| メソッド | 内容 |
|---|---|
| `connectionInfo()` | App APIのloopback endpoint、固定する証明書DER、OAuth origin、OAuth信頼根、node/cluster ID |
| `status()` | ノード・同期状態のJSON |
| `ensureRunning()` | オンデマンド起動（UIDごとに10秒のレート制限） |
| `requestSync()` | アプリ接続を契機とした同期要求（ポリシーは迂回しない） |
| `beginApproval(connectionId)` | 呼び出し元のUID・パッケージ・署名証明書を検証し、単回使用トークンを発行 |
| `nodeSummary()` | 承認UI向けの概要（秘密を含まない） |
- HTTPS App APIはloopback限定のまま。SDKは `Host: harmion.local` を送る必要があるため、
  既存Rustクライアント（`crates/client`）をJNIで再利用し、TLSは証明書DERを厳密に
  固定する。loopbackであることだけを信頼根にしない。
- Peer APIリスナーとApp APIリスナーは分離したまま。
### アプリの識別
- Android UID: `Binder.getCallingUid()` をService内で検証。
- Package name と署名証明書: `PackageManager` から取得し、SHA-256を記録。
  呼び出し元の自己申告は使わない。同一UIDの複数パッケージは同一主体として扱う。
- Harmion App ID: 承認時にCoreがOAuth clientを発行した結果（`client_id`）が主体。
  Android側のパッケージ識別は承認トークンにのみ使い、Coreの認可モデルを変更しない。
- 署名変更・再インストールは別主体として扱われる（証明書ダイジェストが変わるため）。
### 初回接続フロー
1. 利用アプリがSDKの `Harmion.connect()` でServiceへbindする。
2. `connectionInfo()` でendpointと証明書を取得する。
3. SDKが `connect-start`（PKCE S256、state、grants）をApp APIへ送る。
4. `beginApproval` で単回使用トークンを取得する。
5. 承認ActivityをIntentで起動する。Intentに載るのは `connection_id` と短命トークンだけ。
6. 承認Activityは「起動元パッケージ」とトークンの束縛（UID・パッケージ・署名・
   connection_id）を照合し、要求内容を表示する。
7. 許可すると、アプリ内のNode管理資格情報で `connect-approve` を呼ぶ。
8. 利用アプリは `connect-status` をポーリングし、`connect-complete` でトークンを得る。
9. 以後は既存App API（`save`/`get`/`query`/`subscribe`）を利用する。
長期資格情報（request secret、refresh token、管理資格情報）はIntentやActivity結果に
含めない。request secretはSDKのメモリ内のみ、トークンはKeystore暗号化ストレージに置く。
### SDK
- Kotlin: `org.harmion.sdk`（bind、承認、トークン保存・更新、`Flow`購読）。
- データプレーン: `crates/client-ffi` が既存Rustクライアントを公開する。
  HRQ/HQUの正規エンコード、証明書固定、subscribeのカーソル管理を共有するため、
  Kotlinで再実装しない。
- 接続断: Query/Getは再取得、Saveはrequest_idの冪等性契約、Subscribeはresumeを試み、
  だめなら初期取得（`reset_required`）。Interpreter Callは自動再実行しない。
## 8. 認可の有効期限
- SDK接続の承認画面で「30日 / 90日 / 1年」を選べる。`ConnectionApproval.expires_at`
  （任意、unix micros）として渡し、`connect::approve` がAppの絶対期限に設定する
  （`now < expires_at <= now + 10年`）。省略時は従来どおり。
- OAuth familyの新規認可の既定絶対期限は30日（`node/src/oauth.rs`、Core既存）。
  OAuthクライアント行自体は`i64::MAX`で作られるため、管理画面では「無期限」と表示する。
- 既存アプリの期限延長は `change-grants` の任意フィールド `expires_at`（unix micros）で行う。
  `now < expires_at <= now + 10年` のみ受理し、owner assertion（R2）を要求する。
- 管理画面の「権限を管理」では、grant・失効に加えて「変更なし / 30日 / 90日 / 1年」を
  選べる。延長は同じowner承認1回でgrant変更と同時に適用される。
## 9. 所有者認証
所有者承認は、Androidでも`ssh-keygen`なしで完結する。
- 検証: `crates/node/src/sshsig.rs` がOpenSSHのSSHSIGを内製検証する。
  - Ed25519とECDSA P-256（uncompressed SEC1点、r/s mpint）に対応。
  - namespaceは`harmion-owner`固定、hashはSHA-256/SHA-512、SSHSIG構造の
    余剰バイトは拒否。登録済み公開鍵はblob完全一致で照合する。
  - RSA/FIDO2 `sk-*`は未対応として`ssh-keygen -Y verify`へフォールバックする
    （デスクトップの既存挙動を維持）。Androidでは未対応鍵を登録しない運用。
  - 署名チャレンジは単一使用で、同じassertionの再送はノードが拒否する。
- 署名: 所有者seed（32バイト）はアプリが生成し、Android Keystoreの
  AES-GCM鍵（`setUserAuthenticationRequired(true)`、BIOMETRIC_STRONG）で
  暗号化してのみ保存する。署名のたびにBiometricPromptを通し、
  復号したseedを`HarmionNative.ownerSign`へ渡してSSHSIGを組み立てる。
  seedは署名後にネイティブ側で破棄する（文字列としてJNIを1回通過する点は
  残課題）。
- 登録: `NodeRuntimeController.enrollOwner` → 実行中ランタイムのstoreで
  `owner::enroll_initial`を呼ぶ。初回のみ成功し、回復コードを一度だけ返す。
  UIは回復コードをダイアログ表示し（FLAG_SECURE）、再表示しない。
- 回復コード再登録: 設定の「リカバリコードで再登録/復元」→ 新しいseedと公開鍵を生成し、
  App API `owner-recover` に回復コードと公開鍵を送る。ノードは既存owner credentialを
  すべて失効させ、新しいcredentialと**新しい回復コード**を返す。端末側は新seedを
  Keystoreで暗号化して保存し直す。
- 承認: `OwnerAdmin.call`がノードの`owner-status.enforce`を確認し、有効なら
  `owner-challenge` → 生体認証 → `owner-sign` → App APIの対象操作
  （owner assertion付き）を実行する。無効時は管理資格情報のみの呼び出しに
  フォールバックする。現在この経路を使うUIは `connect-approve`、`change-grants`
  （期限延長含む）、`issue-app`（デバッグ操作）。
- 検証済み: ホスト上で「登録→チャレンジ→署名→`connect-approve`→
  `connect-status=approved`→リプレイ拒否」を`android/scripts/owner-smoke.sh`
  で確認する。SSHSIG単体テストは`ssh-keygen`の署名を内製検証で受け、
  逆に内製署名を`ssh-keygen -Y verify`が受理することも確認する。
- 残課題:
  - 物理端末でのKeystore/BiometricPrompt動作確認（エミュレータでは指紋登録と
    `adb -e emu finger touch 1`で確認済み）。
  - join/retire/confirm-retirement等、未接続のR2操作への`OwnerAdmin.call`適用。
  - 回復コードのクリップボード/パスワードマネージャ連携。
## 10. 着信接続
- 主経路はiroh（UDP）。Androidからの発信は必須要件として満たす。
- irohのbindに失敗した場合、HTTPS Peerリスナーを現在のLANアドレスにbindする
  フォールバックを行う（`NodeRuntimeController.startOptions`）。到達可能性は保証しない。
- 既定では着信待ちの常駐をしない。UIはON_STARTでacquireしON_STOPでreleaseする。
  bound serviceはbindを保持してもacquireしない（任意アプリのbind保持で常駐させない）。
  SDKは各操作の前に`ensureRunning`でオンデマンド起動する。ユーザーが「常時稼働」を
  有効にした場合のみforeground serviceが保持し、着信・定期同期を継続する。
  端末再起動後の自動起動はしない。
- 着信・発信とも既存のmembership認可を通る。着信によってポリシーを迂回しない。
## 11. UI・通知
- Jetpack Compose 1.13 / Material3 1.5（Expressive: `LoadingIndicator`、
  `LargeTopAppBar`、`SegmentedButton`、`MotionScheme.expressive()`、
  Dynamic Color、Light/Dark、edge-to-edge）。
- 方針: 説明的な長文を置かず、状態・アイコン・短いラベル・構造で伝える。
  セキュリティ上重要な警告（強い権限、所有者キー失効、回復コード再表示不可）だけを残す。
- 承認Activityは`taskAffinity=""`の専用タスクで開き、完了時に`finishAndRemoveTask()`で
  タスクごと閉じる。これにより承認後はノードアプリではなく要求元アプリへ戻る（F-17の緩和も兼ねる）。
- 画面: Onboarding（作成 / QR参加 / 参加要求QR / コード貼り付け）、
  ホーム（状態・同期・競合・ノード）、ピア（招待QR・参加承認・メンバー）、アプリ（接続要求・認可済みアプリのgrant/期限管理）、
  設定（同期ポリシー・常時稼働・所有者承認・回復コード再登録）、接続承認Activity、
  競合解決シート（候補選択・本文3-way merge）。
- 通知: 同期・承認・競合/ストレージ（`issues`）。競合とストレージ異常は
  `IssueCheckWorker`（30分間隔、WorkManager）が検出して通知し、解消時に取り消す。
  常時稼働中は `KeepAliveService` の継続通知に「稼働中 / 同期待機 / 同期中」とピア数を表示。
- ウィジェット: Glance 1.2（3x2）。状態・ピア数・「開く」「同期」。
  状態はSharedPreferencesに保存した非機密のスナップショットから描画する。
## 12. ビルド
- `flake.nix` の `android` devShell（nixpkgs `2786a2a72088ecf32aea5b3ec52099e6a7cff78f`）:
  Android SDK platform 37.2 / Build Tools 37.0.0 / NDK 30.0.16248370 /
  Gradle 9.9.2（`androidPkgs.gradle_9`）/ JDK 21。
- AGP 9.5.2 はKotlinを内蔵するため `org.jetbrains.kotlin.android` は適用しない。
  Compose Compilerプラグイン（2.4.21）は `:app` のみ。`:sdk` はCompose実行時を含めない。
- compileSdk 37（minor 2）/ targetSdk 36 / minSdk 29。Compose BOM 2026.11.01。
- applicationIdは`dev.satler.harmion`。コードのnamespaceは`org.harmion.android`のまま
  （SDKは`dev.satler.harmion/org.harmion.android.service.HarmionNodeService`をbindする）。
- release署名は1Passwordのkeystore Documentと署名アイテムから取得する
  （[android-release.md](android-release.md)）。
- SDK/エミュレータはnixpkgsから供給し、Gradle/SDKの手動ダウンロードはしない。
- ネイティブ: `nix develop .#android --command ./android/scripts/build-rust.sh arm64-v8a`
  - `libharmion_android.so` → `android/app/src/main/jniLibs/<abi>/`
  - `libharmion_sdk_native.so` → `android/sdk/src/main/jniLibs/<abi>/`
  - minSdk 29。ABIは既定で arm64-v8a（arm64/armeabi-v7a/x86_64を指定可能）。
- APK: `cd android && gradle :app:assembleDebug`（debugはarm64-v8a + x86_64）
- release: `HARMION_KEYSTORE=... HARMION_KEYSTORE_PASSWORD=... HARMION_KEY_ALIAS=... HARMION_KEY_PASSWORD=... gradle :app:assembleRelease`。
  R8/リソース縮小を有効化。SDKのノード証明書ピンは既定でdebug証明書のため、releaseでは
  `-PharmionNodeCertSha256=<release証明書のSHA-256>` を付けてSDKを再ビルドする。
- エミュレータ: API 36 `google_apis` x86_64 AVD。`target/emulator/` に画面証跡。
- JNIのホスト検証: `./android/scripts/jni-smoke.sh`（JVMから同じシンボルをロード）
## 13. docs/android-node.md との差分・未実装

優先度順。

1. 物理端末でのKeystore/BiometricPrompt/Doze/着信の検証（エミュレータでは完了）。
2. 接続承認以外のR2操作（join/retire/confirm-retirement等）への`OwnerAdmin.call`適用。
3. OAuth（ブラウザ）認可時の期限選択: SDK接続は選択可能。OAuth offer側の期限選択は未対応。
4. 競合解決の実Peer間E2E: 2ノードのRust E2Eで、UIが送る`select`/`resolve` JSONと
   `merge_plan`を通した解決まで確認済み。エミュレータ2台での画面操作のみ未実施。
5. QR参加: 参加側の読み取り/貼り付け→`join`→**参加要求QRの表示**、既存ノード側の
   **参加要求の読み取り/貼り付け→owner承認（`approve`）**まで実装。参加要求は
   `PendingJoin`として保存し、ホーム/ピアの「参加承認待ち」カードに永続表示する。
   membershipがactiveになると自動で消去する。既存ノードのメンバー追加まで確認済み。
6. Interpreterのオンデマンド起動・Wasm Runner。
7. 部分同期・履歴削減（Core共通）。
8. クイック設定タイル。

### セキュリティレビュー対応（2026-11-25）
- High: F-01（API 34+のidentity共有）、F-02（FLAG_SECURE・overlay不可・二重検証）は修正済み。
- Medium: 同期ゲート初期許可（Rust唯一のMedium）→ `sync_blocked` と起動前評価で修正。
  F-03（承認UIの同一性・遷移先・grant差分）、F-05（承認tokenの非追い出し・**request_secret
  による接続束縛**）、F-06（throttleの同期化）、F-07（ポリシー初期評価・バッテリー再評価）、
  F-09（refresh Mutexと401競合の再確認）、F-10（API 29の鍵無効化・回復UI）も対応。
  F-08はSDKがノードアプリの署名SHA-256を`BuildConfig.NODE_CERT_SHA256`で照合する。
- F-04（Service認証）: 承認完了時に`CallerAccess`へpackage/署名証明書/app_idを記録し、
  `status`/`requestSync`/`nodeSummary`は承認済みパッケージのみ許可。`beginApproval`は
  `request_secret`を`connect-status`で検証し、`registerCaller`は接続完了後にのみ登録する。
  未承認の`connectionInfo`はOAuth trust materialを返さない。bind保持はacquireせず、
  UIはSTART/STOPで参照を管理。UID/グローバル配额とレート制限を追加。
- 残: `connectionInfo`と`ensureRunning`は初回接続のブートストラップのため無認証のまま
  （レート制限・最小情報・グローバル起動ゲートで緩和）。F-14の配布APKはdebugのまま
  （release設定は追加済み、署名鍵は環境変数）。
## 14. テスト

- `cargo test -p harmion-runtime`: 初期化・起動・停止・再起動、2ノードの参加・承認・
  双方向同期（pinned HTTPS App API経由）。daemonの旧スケジューラ試験（順序・
  バックオフ・回復）も移植して検証する。
- `cargo test -p harmion-node --lib sshsig`: Ed25519/ECDSA P-256のSSHSIG検証、
  `ssh-keygen`署名の受理、内製署名の`ssh-keygen -Y verify`受理、
  `ssh-keygen`無しでの固定fixture検証、RSAのフォールバック判定、不正入力の拒否。
- `cargo test -p harmion-android`: JSON API（initialize/start/status/sync/policy/stop）
  と、所有者鍵のenroll→署名→ノード検証（enforce含む）。
- `cargo test -p harmion-sdk-native`: 保存要求の正規エンコード、条件JSON、エラー、
  `select`/`resolve`スペックのパース（UIが送るJSON形状）。
- `cargo test -p harmion-net-native --test https_sync ui_shaped`: 2ノードで実際に本文競合を作り、
  Android UIと同じ`select`/`resolve` JSONと`merge_plan`で解決するE2E。
- `cargo test -p harmion-store-sqlite --test application`: `apps()`の管理読み出し権限、
  `change-grants`の期限延長（過去/過大の拒否）、Select/Resolve、grant世代。
- `android/scripts/jni-smoke.sh`: ノードcdylibをJVMから呼ぶ（JNIシンボルと初期化）。
- `android/scripts/sdk-smoke.sh`: ノードを1つのJVMで起動し、別JVMのSDK cdylibから
  pinned HTTPS経由で save/get/query/subscribe（スナップショットと変更通知）を検証する。
  Androidのプロセス分離に合わせて2プロセスで実行する。
- `android/scripts/owner-smoke.sh`: 同一JVMでノードcdylibとSDK cdylibを使い、
  owner-enroll → owner-challenge → owner-sign → `connect-approve`（assertion付き）
  → `connect-status=approved` → リプレイ拒否を検証する。検証は内製実装のみで、
  `ssh-keygen`は使わない。
- `gradle :app:assembleDebug`: Kotlin/AIDL/ComposeのコンパイルとAPK生成。
- daemonの実プロセス検証（`design_checks/two_process_sync.py`、`oauth_process.py`、
  `connect_process.py`、`restore_process.py`、`admin_process.py`、
  `multi_process_sync.py`、`membership_process_sync.py`）は、daemonを
  `crates/runtime` へ移行した後も同じCLI・同期・認可・復元挙動を確認する。

JNI実装上の注意（実測）: `JString` を `JavaVM` シングルトン経由で読むと、プロセス内で
最初のネイティブ呼び出し（初回起動の `initialize`）だけ失敗する。両cdylibとも
`EnvUnowned::with_env` が渡す `Env` から `mutf8_chars` で読む。この修正後は
両cdylibを同一JVMにロードしても動作する（`owner-smoke.sh`で確認）。

エミュレータ検証（API 36 x86_64）:
- 初回作成→ホーム、同期ポリシー（充電待ちで停止）、所有者enroll（指紋）→enforce有効。
- 常時稼働ON→FGS通知「稼働中・ピア0台」、HOME後もプロセス継続。
- アプリ一覧（`apps`）→ grant管理シート→90日延長（owner承認）→期限更新を確認。
- 回復コード再登録→新コード発行・新fingerprint・enforce有効を確認。
- Glanceウィジェットをランチャーに配置→「稼働中 / 0 ピア / 開く / 同期」を描画。
- F-04検証: 未承認の`status`が拒否されること、承認後は`status`が通り、HOME+40秒で
  `running=false/stopped`、次の`createContent`でオンデマンド起動して保存できることを確認。
- 参加承認E2E（2 Androidユーザー）: Aを作成＋所有者登録、Bが招待コードを貼り付けて`join`、
  Bの参加要求コードをAの「参加要求を承認」に貼り付け→指紋→Aのメンバーが2台になることを確認。
- 「参加承認待ち」カード: Bで参加後にホーム/ピアへ表示され、アプリを強制終了→再起動しても
  同じQRが再表示されることを確認。保存コードのrelay_urlを意図的に古い値へ書き換えて起動し、
  現在の`identity()`の値（relay_url=null）へ自動更新されることも確認。
- 承認後の戻り先: 承認完了直後のtop activityが`org.harmion.testclient/.MainActivity`で
  あること、ApprovalActivityのタスクが残らないことを確認。
- 承認E2E: debug専用`android/testclient`（`:testclient`）から接続要求→承認画面
  （パッケージ・証明書・リダイレクト・期限30/90/365）→90日選択→指紋→connectComplete→
  createContentまで確認。ノードのアプリ一覧に90日期限で反映。
- `target/emulator/` に各画面のPNGとUIダンプを保存。`testclient`はデバッグ専用で配布しない。

物理端末（未実施）: Wi-Fi/モバイル切替、Doze、プロセス終了、機種変更、着信接続、
長時間のバッテリー・ストレージ挙動。要求文書の受け入れ条件は実機で確認する。
