# native 複数ノード同期の実行

Unix用のnative管理CLIで、署名付き参加・退役・却下証明をノード間で伝播する。Query/Subscribeと、任意で有効化するブラウザOAuthを提供する。InterpreterとDOは未実装。CLIは所有者限定ファイルから接続先と管理資格情報を読み、証明書を固定したloopback HTTPSで操作する。

## 起動と参加

```sh
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

```sh
target/debug/harmion --node ./node-a approve ./node-b/peer.json
target/debug/harmion --node ./node-b join ./node-a/peer.json
target/debug/harmion --node ./node-a sync NODE_B_ID
```

`peer.json`は公開の本人確認鍵・接続情報・クラスタ共通の初期信頼証明を含む登録JSON v3。別端末では所有者が信頼する経路でファイル全体を確認する。AでBの参加を承認し、Bはjoinで既存クラスタと連絡先を確認する。join前や参加証明の受信前はデータ同期できない。CLIのjoinはB自身のpeer.jsonも更新する。

3台目以降も、有効な1台でapproveし、新端末でその端末へjoinする。同期すると他ノードも参加証明と接続情報を学習するため、全組合せの承認は不要。新しく参加したノードで次のノードを承認する前に、同期して自分の参加を確認する。最初の端末の常時起動は不要。

`admin.secret`・`private-key.der`・`iroh.secret`は交換しない。管理資格情報は所有者bootstrap用で、OAuth access tokenではない。

起動時にSQLite DBをschema v9へ移行する。旧DBの検索索引と項目の版参照は保持している更新から作る。Subscribeの変更履歴には移行後の更新を記録する。schema v4以前から移行した旧クラスタでは、起点にする1台だけを所有者が選び、`run DIR --bootstrap-cluster-root true`で共通の初期信頼証明を作る。他ノードは新しいv3登録でapprove/joinし直す。無署名だった既存Peerを、署名検証鍵を確認せず自動的に他端末へ紹介しない。データ・アプリ資格情報・データcursorは維持する。

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

自ノードからの自動同期は起動時、Contentの新規保存が確定したとき、および周期照合で行う。周期は標準30秒で、`--sync-seconds 1`等で変更できる。0では自ノードからの開始は手動操作に限り、認証済みPeerからの同期は受け付ける。到達不能なPeerは保存後も最大1時間のbackoffとjitterに従い、状態が変化した時だけstderrへ記録する。SIGINT/SIGTERMで接続taskと同期taskを停止する。再起動後もID・鍵・ログ・cursor・編集token・再送結果を保持する。

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

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
python3 design_checks/two_process_sync.py
python3 design_checks/restore_process.py
cargo build -p harmion-net-native --example local_relay
python3 design_checks/two_process_sync.py --iroh
python3 design_checks/multi_process_sync.py --iroh
python3 design_checks/membership_process_sync.py --iroh
```

引数なしのテストはHTTPS経路を明示的に選び、一時directoryに独立したdaemonを2つ起動し、CLI経由で分断編集、競合解決中の追加更新、削除競合、クラッシュ後の再送、周期同期、退役を検証する。localhostの待受け権限が必要。

`--iroh`は同じCLIシナリオをirohで実行し、localhostの公式リレーと公開探索なしの設定を使う。irohのIP変更・探索情報更新と直接IP transportなしのリレー同期はRustの`iroh_sync`テストで検証する。実インターネットのNAT環境・公開探索サービスの可用性はこのローカルテストでは検証しない。

multi_process_syncは5台の中継・循環・分断・再起動・周期同期、membership_process_syncは参加・退役の伝播、確認済み子ノードの維持、相互退役の保留・第三ノードの決定、所有者回復、後着退役証明の明示的確定を扱う。どちらも`--iroh`を外すとHTTPS経路を使う。
