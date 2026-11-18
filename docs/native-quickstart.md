# native 複数ノード同期の実行

現時点はUnix用のnative管理CLI。署名付き参加・退役・却下証明をノード間で伝播する。OAuth、ブラウザ、query/subscribe、Interpreter、DOは未実装。CLIはDBを開かず、所有者限定ファイルから接続先と管理credentialを読み、証明書を固定したloopback HTTPSで操作する。

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

起動時にSQLite DBをschema v8へ移行する。旧DBの検索索引と項目の版参照は保持している更新から作る。Subscribeの変更履歴には移行後の更新を記録する。schema v4以前から移行した旧クラスタでは、起点にする1台だけを所有者が選び、`run DIR --bootstrap-cluster-root true`で共通の初期信頼証明を作る。他ノードは新しいv3登録でapprove/joinし直す。無署名だった既存Peerを、署名検証鍵を確認せず自動的に他端末へ紹介しない。データ・アプリ資格情報・データcursorは維持する。

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

## 検証

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
python3 design_checks/two_process_sync.py
cargo build -p harmion-net-native --example local_relay
python3 design_checks/two_process_sync.py --iroh
python3 design_checks/multi_process_sync.py --iroh
python3 design_checks/membership_process_sync.py --iroh
```

引数なしのテストはHTTPS経路を明示的に選び、一時directoryに独立したdaemonを2つ起動し、CLI経由で分断編集、競合解決中の追加更新、削除競合、クラッシュ後の再送、周期同期、退役を検証する。localhostの待受け権限が必要。

`--iroh`は同じCLIシナリオをirohで実行し、localhostの公式リレーと公開探索なしの設定を使う。irohのIP変更・探索情報更新と直接IP transportなしのリレー同期はRustの`iroh_sync`テストで検証する。実インターネットのNAT環境・公開探索サービスの可用性はこのローカルテストでは検証しない。

multi_process_syncは5台の中継・循環・分断・再起動・周期同期、membership_process_syncは参加・退役の伝播、確認済み子ノードの維持、相互退役の保留・第三ノードの決定、所有者回復、後着退役証明の明示的確定を扱う。どちらも`--iroh`を外すとHTTPS経路を使う。
