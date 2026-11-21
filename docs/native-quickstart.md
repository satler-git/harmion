# native 複数ノード同期の実行

Unix用のnative管理CLIで、署名付き参加・退役・却下証明をノード間で伝播する。Query/Subscribeと、任意で有効化するブラウザOAuthを提供する。InterpreterとDOは未実装。CLIは所有者限定ファイルから接続先と管理資格情報を読み、証明書を固定したloopback HTTPSで操作する。

## 起動と参加

```sh
npm ci --prefix apps/admin/ui
cargo build --workspace
target/debug/harmiond init ./node-a
```

出力の`cluster_id`を使って同じクラスタのBを初期化する。初期化先は存在しないdirectoryを指定する。途中で初期化が失敗したdirectoryは自動的に再初期化しない。

```sh
target/debug/harmiond init ./node-b --cluster CLUSTER_ID
```

それぞれ別terminalで起動する。

```sh
target/debug/harmiond run ./node-a
target/debug/harmiond run ./node-b
```

新規ノードのPeer通信はiroh。`iroh.secret`を0600で保持し、再起動後も同じEndpoint IDを使用する。鍵ファイルが失われた場合は起動を拒否し、自動再生成しない。相手のIPを登録する必要はない。標準はn0の公開リレー・DNS/Pkarr探索を使う開発用構成。公開リレーの本番利用は推奨されない。

自前リレーは`--relay-url https://relay.example.com`で選べる。設定はnode.jsonへ保存され、`relay_urls`に複数URLを指定することもできる。リレーだけを自前にしても標準ではDNS/Pkarr探索はn0を使う。`--discovery none`と明示的な`--relay-url`を併用すると公開探索を使わず、peer.json内のリレー経由でEndpoint IDへ接続する。この構成でリレーURL自体を変えた場合はpeer.jsonを更新して再承認する。iroh endpointのbind完了と、リレー接続・公開探索への反映完了は別であり、起動直後のsyncは未接続で失敗する場合がある。周期同期は再試行する。

Appのloopback HTTPSは初回に空きポートを選び、以後同じポートで再起動する。`--app-listen IP:PORT`で変更可能。HTTPS Peerの検証経路は`--peer-transport https --peer-listen IP:PORT`で選ぶ。この経路だけはIP変更時に相手へ再登録が必要。既存node.jsonでuse_irohがない場合はHTTPSを維持する。承認済みNode IDの鍵を別方式の鍵へ差し替える操作は未実装で、切替後の再承認は拒否される。

signed-HTTPSのPeerが所有者提供のprivate CA配下にいる場合は、そのCA証明書（DER）を`--signed-https-root /path/to/ca.der`で追加できる（繰り返し指定可）。WebPKIのchain・hostname・期限・server-auth検証は無効化されず、追加rootはtrust anchorとしてだけ使われる。通常のCloudflare公開証明書では不要。

```sh
target/debug/harmion --node ./node-a approve ./node-b/peer.json
target/debug/harmion --node ./node-b join ./node-a/peer.json
target/debug/harmion --node ./node-a sync NODE_B_ID
```

`peer.json`は公開の本人確認鍵・接続情報・クラスタ共通の初期信頼証明を含む登録JSON v3。別端末では所有者が信頼する経路でファイル全体を確認する。AでBの参加を承認し、Bはjoinで既存クラスタと連絡先を確認する。join前や参加証明の受信前はデータ同期できない。CLIのjoinはB自身のpeer.jsonも更新する。

3台目以降も、有効な1台でapproveし、新端末でその端末へjoinする。同期すると他ノードも参加証明と接続情報を学習するため、全組合せの承認は不要。新しく参加したノードで次のノードを承認する前に、同期して自分の参加を確認する。最初の端末の常時起動は不要。

`admin.secret`・`private-key.der`・`iroh.secret`は交換しない。管理資格情報は所有者bootstrap用で、OAuth access tokenではない。

起動時にSQLite DBをschema v12へ移行する。同期状態の新しい観測領域は空で作り、過去の同期成功を推測で補わない。既知署名identityのpinは、現に有効な参加者とtrust rootだけを移行時にseedし、鍵を検証できない旧 peer は unknown のまま残す。旧DBの検索索引と項目の版参照は保持している更新から作る。Subscribeの変更履歴には移行後の更新を記録する。schema v4以前から移行した旧クラスタでは、起点にする1台だけを所有者が選び、`run DIR --bootstrap-cluster-root true`で共通の初期信頼証明を作る。他ノードは新しいv3登録でapprove/joinし直す。無署名だった既存Peerを、署名検証鍵を確認せず自動的に他端末へ紹介しない。データ・アプリ資格情報・データcursorは維持する。署名HTTPS（登録v4）は[署名HTTPSのdraft](../contracts/https-peer-v2-draft.md)を参照する。

## Linuxのユーザーサービスとして運用する

所有者が任意で導入するsystemdユーザーサービスと、release build・更新・停止したノードのバックアップ・新しいNode IDでの復元手順は [Linuxの日常運用](linux-service.md) を参照する。ビルドやテストだけでホストへサービスを登録・起動する処理はない。ログアウト後の常駐やlingerは別途所有者が判断する。

## OAuthを有効にする

ノードを停止した状態で、所有者が専用CAとサーバー証明書を発行する。初期状態ではOAuthの待受けを無効にしている。

```sh
target/debug/harmiond oauth-provision ./node-a > node-a-oauth-root.pem
target/debug/harmiond run ./node-a --oauth-listen 127.0.0.1:0
```

標準出力の `oauth_origin` にHTTPS originが表示される。ポートは再起動後も維持する。公開CAを再取得するには `harmiond oauth-root ./node-a` を使う。サーバーの秘密鍵を含む `oauth-identity.json` はノードディレクトリー内に保管する。

認可の前に、所有者が生成した `node-a-oauth-root.pem` を、使うブラウザのWebサイト用CAとしてインポートする。Firefoxでは設定の「プライバシーとセキュリティ」から「証明書を表示」 → 「認証局証明書」 → 「インポート」と進み、Webサイトの識別を信頼する。ブラウザがOSの証明書ストアを使う環境では、利用ユーザーの信頼ストアを使う。token endpointを呼ぶアプリにも同じPEMを設定する。App APIにはSDKの証明書固定接続を使う。

```sh
target/debug/harmion --node ./node-a oauth-register "My app" http://127.0.0.1/callback
target/debug/harmion --node ./node-a oauth-consent REQUEST_ID
```

アプリは登録時に返された認可URLを [App APIのOAuth契約](app-api.md#5-oauthとローカル構成) に沿って開く。2つ目のコマンドは、所有者がブラウザに表示された要求IDを確認して実行する。表示されたリンクは所有者が承認または拒否のためにそのブラウザで開く。Rust SDKでのApp API接続には、ノードディレクトリーとaccess tokenを `Client::from_directory_with_token` に渡す。

証明書の有効期間は発行から1年。期限前にノードを停止し、`harmiond oauth-provision ./node-a --replace > node-a-oauth-root.pem` で入れ替える。新しいPEMをブラウザとアプリに登録し、古いCAを信頼ストアから削除する。OAuthの待受けを停止するには `run DIR --oauth-listen off` を使う。発行済み資格情報はApp APIの `change-grants` で失効できる。

## 作成・取得・再送

```sh
printf 'hello\n' > body.txt
target/debug/harmion --node ./node-a create body.txt ./node-a/create-1.bin note
target/debug/harmion --node ./node-a sync NODE_B_ID
target/debug/harmion --node ./node-b get CONTENT_ID
```

createの出力に`content_id`と結果viewが含まれる。getの`edit_token`で編集する。

```sh
target/debug/harmion --node ./node-a edit CONTENT_ID EDIT_TOKEN body.txt ./node-a/edit-1.bin
```

保存要求ファイルは送信前に0600で新規作成される。親directoryも所有者限定（0700等）でなければ拒否する。要求ファイルの上書きはしない。タイムアウトや接続断で成功不明の場合は、同じファイルを再送する。

```sh
target/debug/harmion --node ./node-a save ./node-a/edit-1.bin
```

再送結果は元の保存時点の結果であり、現在値の取得とは異なる。credential失効・現在の権限も再検査する。

## 保存更新の受領確認

保存成功はこの端末への永続化を意味する。別端末への受領確認は、保存結果の `update_id` で別途取得する。

```sh
target/debug/harmion --node ./node-a sync-status
target/debug/harmion --node ./node-a sync-status --update UPDATE_ID
```

所有者用の読み取り操作で、ネットワーク接続は行わない。Peer ごとの `last_success_at` は最後に完了した自発同期の確認時刻、`last_failure` は最新の試行が失敗した場合の分類と時刻。成功すると失敗表示を消し、失敗しても以前の受領確認は保持する。手動の `sync NODE_B_ID` と自動同期の両方が観測を更新する。

`update_confirmed: true` はそのPeerで当該更新の受領を確認済み。`false` は観測位置が足りず未確認、`null` は更新未指定または有効な観測なし。どちらも未受領とは断定できない。`unconfirmed_local` は確認したログ位置に含まれない更新件数であり、中継更新も含む。初回同期で取得した更新は、次の同期まで未確認に残ることがある。

初回・旧DB移行直後は観測なし。ノード再起動後も確認を保持するが、新しい Node ID でのバックアップ復元先には引き継がない。現在のオンライン状態、第三ノードへの伝播や将来のバックアップを保証する表示ではない。参加・保留状態は `members` も確認する。取得権限・各フィールドと失敗コードの詳細は [同期状態のAPI契約](app-api.md#所有者向け同期状態更新の受領確認) を参照。

## 競合と停止

ノードの参加状態は`members`で確認する。通常の退役は有効な端末で`retire NODE_ID`を実行し、他端末へ同期する。承認元が退役しても、維持対象に入った別ノードの承認は残る。

相互退役や発行元の退役で確認できなくなった退役証明は`pending_retirements`へ出る。対象は`conflict`となりデータ同期を停止する。`sync`が退役・競合エラーを返しても、その前段の管理証明はcommit済みの場合がある。`members`で現在の状態を確認する。

独立して有効な端末で、各証明IDに対して次のいずれかを選ぶ。

```sh
target/debug/harmion --node ./active-node confirm-retirement CERTIFICATE_ID
target/debug/harmion --node ./active-node dismiss-retirement CERTIFICATE_ID
```

前者は新たな有効な退役証明と古い保留証明の却下を原子的に保存し、後者は保留証明だけを却下する。有効な退役証明そのものの取り消しには使えない。相互退役でAを残してBを退役させるなら、Bを対象とする証明を確定し、Aを対象とする証明を却下する。

独立した有効ノードが使えない場合は、新しいNode IDの回復ノードを同じclusterで作り、所有者が次を行う。回復用trustは自動伝播しないため、各対象端末で公開鍵・登録ファイル全体を別経路確認する。

```sh
target/debug/harmion --node ./recovery join-recovery ./node-a/peer.json
target/debug/harmion --node ./node-a trust-recovery-peer ./recovery/peer.json
target/debug/harmion --node ./node-b trust-recovery-peer ./recovery/peer.json
target/debug/harmion --node ./recovery approve ./node-a/peer.json
target/debug/harmion --node ./recovery approve ./node-b/peer.json
```

回復ノードから双方へsyncして証拠を集め、membersの証明IDを確定／却下した後、もう一度syncする。同期先が保留・退役中ならデータ同期部分のエラーは想定される。状態を見ずに一括再承認して回避しない。詳細は[参加・退役証明の契約](../contracts/membership-v1.md)を参照。

`conflicts CONTENT_ID`の`view.edit_token`と本文候補の`versions`を使い、`resolve CONTENT_ID TOKEN BODY_FILE REQUEST_FILE VERSION_ID...`で明示した候補を置換する。未観測の並行更新は保持する。

削除は`delete CONTENT_ID TOKEN REQUEST_FILE`。管理`inspect CONTENT_ID`で削除済みContentを取得でき、`keep`で削除競合の生存を選べる。`restore CONTENT_ID TOKEN BODY_FILE REQUEST_FILE`は既知フィールドを置換する操作で、このCLIコマンドは本文しか指定しないため、既知の属性・kindを消す。属性を含む復元はSDKのSaveRequestで全フィールドを明示する。

自ノードからの自動同期は起動時、Contentの新規保存が確定したとき、および周期照合で行う。周期は標準30秒で、`--sync-seconds 1`等で変更できる。0では自ノードからの開始は手動操作に限り、復帰監視も起動しない。認証済みPeerからの同期は受け付ける。到達不能なPeerは通常は保存後も最大1時間のbackoffとjitterに従い、状態が変化した時だけstderrへ記録する。SIGINT/SIGTERMで接続taskと同期taskを停止する。再起動後もID・鍵・ログ・cursor・編集token・再送結果を保持する。

### 復帰・ネットワーク変化による再試行

自動同期が有効なら、Linuxでは `CLOCK_BOOTTIME` と `CLOCK_MONOTONIC` の差の増加から1秒以上のサスペンドを推定する。別途、実行間隔が5秒以上空いた場合はプロセス一時停止や実行遅延からの再開ヒントとして扱い、OSのサスペンドと区別する。時刻変更は検知に使わない。

ネットワーク監視はローカルのインターフェース・アドレス・経路の状態変化を通知し、5秒ごとのローカル状態再確認も行う。irohはrelayの新たな接続も通知する。これらは到達可能になった保証ではなく再試行の合図。同じアドレスのまま上流だけが復旧する等、検知できない障害には周期照合を残す。監視を初期化できなくても同期を停止せず、30秒後に監視を再試行する。

ヒントは2秒待って集約し、長いbackoff中でも最大30秒に1回の再試行波を許す。待機中の追加ヒントで期限を延ばし続けず、同期中に届いたヒントは次の1ラウンドに保持する。失敗回数はリセットせず、復旧していなければ通常のbackoffに戻る。同時4Peer・待機順の割り当て・認証と退役検査は維持する。`RUST_LOG=harmiond=debug` でヒントと再試行を確認できる。

## 管理 Web アプリ

フロントエンドは Svelte・TypeScript・Vite で構成する。開発時は Node.js 22.12以降または24系と npm を使い、初回に `npm ci --prefix apps/admin/ui` を実行する。Cargo が UI をビルドして管理アプリの実行ファイルに同梱する。利用時には Node.js プロセスは不要。型チェックは `npm --prefix apps/admin/ui run check` で実行できる。

UI を更新した後は `cargo build --workspace` を実行し、起動中のノードと管理アプリを停止して再起動する。既存のノード・管理アプリの状態ディレクトリを指定する。

`harmiond` を起動した状態で、管理アプリ専用の CA を一度発行する。管理アプリの秘密鍵は新しい所有者専用ディレクトリに格納される。

```sh
target/debug/harmion-admin provision ./admin-ui > harmion-admin-root.pem
target/debug/harmion-admin run ./node-a ./admin-ui
```

ブラウザに `harmion-admin-root.pem` を Web サイト用 CA として登録し、端末に表示された単回リンクを開く。リンクは2分、セッションは30分有効。再読込後は、新しいセッションを作るために管理アプリを再起動する。特定の Content を開く場合は `run` の末尾に Content ID を指定する。公開 CA は `harmion-admin root ./admin-ui` で再取得できる。OAuth の認可画面を開く場合は、[ノードの OAuth 専用 CA](#oauthを有効にする) も登録する。

画面はダークテーマを標準とし、概要・コンテンツ・競合解決・Peer・アプリ・Export を分ける。コンテンツ画面で本文と属性を読み、競合解決画面で候補を比較・編集・選択する。画面の切替中も編集内容と保存の再試行要求を保持する。ノードの管理資格情報は管理アプリのプロセスが使用する。 [管理 Web アプリの契約](specification.md#64-ローカル管理-web-アプリ)

概要にはローカルの Content 数、受理更新数、レコードのバイト数、有効なアプリ登録数を表示する。「最新の状態に更新」で再取得する。参加状態の Active は到達可能性の表示とは分け、手動同期の結果に送受信件数と実行時刻を表示する。

Peer 画面では自身の接続情報を QR と接続コードで共有できる。相手の管理画面でカメラ・QR画像・貼り付けのいずれかで読み込み、操作・Cluster ID・Node ID・署名公開鍵を確認して承認する。読み取りでは参加状態は変化しない。確認は2分有効で1回限り。相手と同じ Cluster ID で初期化し、既存ノードで相手の参加を承認した後、新ノード側で「このノードを相手のクラスタに参加させる」を行う。登録情報が QR の容量を超える場合は接続コードをコピーする。

アプリ画面は1行の要求コードを受け取る。OAuth の認可画面に表示された `harmion-auth:v1:REQUEST_ID` を貼り付けると、ノードに保存されたアプリ名・コールバック・要求権限を表示する。その後に準備した認可画面で許可または拒否する。初回のアプリ登録には `harmion-app:v1:BASE64URL` を使う。`BASE64URL` は `{"name":"My app","redirect_uri":"http://127.0.0.1/callback"}` の UTF-8 JSON をパディングなしで符号化した値。登録結果の client ID と endpoint を含む1行の JSON を要求元へコピーする。

CLI で本文の共通祖先とマージ案を取得する場合は、`inspect` の管理トークンと、表示された本文候補の版 ID を2つ指定する。

```sh
target/debug/harmion --node ./node-a merge-base CONTENT_ID TOKEN LEFT_VERSION RIGHT_VERSION
target/debug/harmion --node ./node-a merge-plan CONTENT_ID TOKEN LEFT_VERSION RIGHT_VERSION
```

共通の本文版が複数なら候補 ID が返る。利用する版 ID を最後の引数に指定できる。案を確認した後、既存の `resolve` で保存する。属性・kind を CLI から選択するには `select CONTENT_ID TOKEN SELECTIONS_JSON REQUEST_FILE` を使う。`SELECTIONS_JSON` は `field`・`version`・`replaces` のオブジェクトを並べた配列。例は [App API の競合解決](app-api.md#競合候補の選択と本文の三者マージ) を参照。

## 古いノードバックアップからの復元

停止したノードのディレクトリのバックアップから、受理済みのContent履歴を新しいノードに復元する。手順では復元先に新しいNode ID・ログ世代・鍵を作る。元のNode IDと受領済みログ位置を再利用すると、相手が更新を受領済みと扱うことがある。

```sh
target/debug/harmiond restore ./offline-backup ./restored-node
target/debug/harmiond run ./restored-node
```

復元先は未作成のディレクトリを指定する。入力はノードディレクトリのバックアップ。復元するデータは、更新IDを維持した本文・属性・kind・競合・削除を含む受理済み履歴。資格情報、アプリ登録、同期カーソルと参加状態は新しいノードのものを使う。OAuthが必要なら新しいノードで設定する。

既存クラスタへは、有効なノードで `approve`、復元先で `join` してから `sync` する。この同期でバックアップにだけあった更新とクラスタ側にだけあった更新も突き合わせる。有効なノードが残っていない場合でもローカルのContentは復元できる。元クラスタへの再参加には有効な承認元が必要。

## Export

新しい出力ディレクトリを指定して管理者用 CLI から実行する。

```sh
target/debug/harmion --node ./node-a export ./harmion-export
```

本文は `bodies/`、現在値・候補・削除記録は `metadata.jsonl` に入る。完了後に `manifest.json` が作られる。ディレクトリとファイルの読み書きは所有者に限定する。 [Export v1の形式](../contracts/export-v1.md)

## 検証

Nixを使う場合は `nix develop` でRust（wasm32 std込み）・node/npm・Pythonが揃う。以下のコマンドはそのshell内で実行する。`flake.nix`・`flake.lock`はgitの追跡対象に含める必要がある。

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
python3 design_checks/two_process_sync.py
python3 design_checks/restore_process.py
python3 design_checks/admin_process.py
cargo build -p harmion-net-native --example local_relay
python3 design_checks/two_process_sync.py --iroh
python3 design_checks/multi_process_sync.py --iroh
python3 design_checks/membership_process_sync.py --iroh
```

引数なしのテストはHTTPS経路を明示的に選び、一時directoryに独立したdaemonを2つ起動し、CLI経由で分断編集、競合解決中の追加更新、削除競合、クラッシュ後の再送、周期同期、退役を検証する。localhostの待受け権限が必要。

`--iroh`は同じCLIシナリオをirohで実行し、localhostの公式リレーと公開探索なしの設定を使う。irohのIP変更・探索情報更新と直接IP transportなしのリレー同期はRustの`iroh_sync`テストで検証する。実インターネットのNAT環境・公開探索サービスの可用性はこのローカルテストでは検証しない。

multi_process_syncは5台の中継・循環・分断・再起動・周期同期、membership_process_syncは参加・退役の伝播、確認済み子ノードの維持、相互退役の保留・第三ノードの決定、所有者回復、後着退役証明の明示的確定を扱う。どちらも`--iroh`を外すとHTTPS経路を使う。
