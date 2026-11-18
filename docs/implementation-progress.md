# 2ノード同期までの実装・レビュー記録

これは段階ごとの履歴であり、以下の「残作業」「未完了」は各記録時点の状態を示す。現在の仕様との差異・暫定対応・未実装項目は[実装全体レビュー](implementation-review.md)を参照。後続の参加・退役・却下証明の実装は[専用契約](../contracts/membership-v1.md)に記録する。

目標: 永続化された2つのノードが認証済み経路で双方向同期し、分断・再送・再起動後も競合と削除を保持して収束すること。段階ごとに設計との照合、テスト、不正入力・障害境界のレビューを行う。

完了条件の再確認: native端末のIPは流動するため、固定IPのHTTPSとlocalhostの2プロセス検証だけでは端末間同期の完成ではない。仕様どおりnative間iroh、HTTPノードへHTTPSとする。安定したEndpoint IDへの認証・探索・再接続と、経路が変わってもNode ID/カーソルを保持する検証が必要。

## 作業順序と完了条件

1. domainとContent — 実装済み。更新グラフ、通常値、固定snapshot、属性値。既存15テスト。
2. protocol — 更新record v1実装済み。canonical bytes、SHA-256、制限、独立Python fixture、不正形式拒否の6テスト。Peerメッセージは次段階。
3. SQLiteとnode保存境界 — 更新・受理ログ・Peerカーソル・固定編集元・アプリ再送結果の原子性と再起動復元を実装。
4. ローカルAPIとCLI — nodeの認証・認可、基本Content use case、native HTTPSホストとSDK、Unix保護ファイル操作、daemon初期化/起動とCLIを実装。OAuthは未実装。
5. Peer同期とHTTPホスト — 世代付きカーソル・ページ原子性・相互TLSのHTTPSホスト・直接相互承認と退役・daemon周期同期を実装。第三ノードへの管理証明の伝播は未実装。
6. 統合検証 — 実DB・独立2プロセスで分断中の編集、再送、クラッシュ再起動、削除競合、解決中の追加更新、有限の中継、周期同期を検証。最終レビュー継続中。

本記録のチェック完了だけで目標達成とせず、実装と実行結果を根拠に判定する。DOの保存境界も早期に適合確認し、未検証なら明示する。

## レビュー: domainからprotocolへの境界

- 更新の依存・root・固定basis・復元の範囲検証はdomainで行う。
- domainのBTreeMap/BTreeSetへ変換する前に、codecが重複を検出する必要がある。
- 数値の同値表現は1つのbytesに正規化する。受信時に再符号化して黙って直さず、非canonicalな入力は拒否する。
- ContentSnapshotには非公開のkind競合やbasisがあるため、一般App APIへそのままserializeしない。
- 次段階の保存はRecordの元bytesを保持し、同ID別bytesをdigestだけで判定しない。

## レビュー: record v1

- 独立したPython生成の322 bytesとSHA-256がRustの出力と一致。
- 全prefix切断、未知version/tag、不正UTF-8、末尾bytes、重複field/object/ID、非canonical数値・順序を拒否。
- 双方向でbyte数・深さ・全体の要素予算を適用。長さの宣言に基づいて巨大な配列を先行確保しない。
- 現時点のcodecは依存・actionの意味を検証しない。必ず保存トランザクション内でdomain検証を通す必要がある。
- byte変異で受理できた入力は、再符号化して完全に同じbytesになることを確認。
- 残作業: SQLite保存と同ID不一致、トランザクション失敗、Peerのページ境界。

## レビュー: SQLite保存とページ受理

- nodeは同期transaction callbackのportを所有し、SQLite adapterがBEGIN IMMEDIATEとcommit/rollbackを所有する。nodeからSQLiteへの依存はない。
- 正本bytesとdigest、受理seq、Peerカーソルを同じDBトランザクションで保存。現在値は全ログから再計算する参照実装で、索引最適化は未実施。
- startupでdigest・update_id・依存順を検査し、破損したレコードを黙って飛ばさない。既存DBのcluster/node/epochは上書きしない。
- 実ファイルSQLiteで再起動、再送、同ID別bytes、digest破損、失敗ページの正常prefix、カーソル保存時のSQL障害、callbackエラーとpanicのrollbackを検証。
- seq欠番、ACK喪失、epoch変更の全走査、sender別カーソル、逆順・重複seq・不正範囲を検証。
- 2つのSQLite DBの双方向転送で、分断中の並行編集、解決中の追加更新、削除と編集の競合、有限回の中継後のログ件数とsnapshot一致を検証。これは同一プロセス内のテストであり、HTTP同期の完成ではない。
- ページ件数上限、固定high後の追加更新、同じDBへの別接続による古いafterの拒否を検証。
- 未完了: network-facing serviceの認証・参加資格、App保存認可・編集トークン・request_id、wire Peerメッセージ、実プロセス間通信。現段階のtransaction-level関数を外部へ直接公開してはならない。
- SQLiteの起動・更新は[rusqlite transaction契約](https://docs.rs/rusqlite/0.40.2/rusqlite/struct.Transaction.html)を参照。エラー時rollbackと明示commitに加え、実DBで障害を注入して確認する。

## レビュー: アプリ保存・認可・再送（schema v2）

- Create/Edit/Resolve/Delete/Keep/Restoreをアプリ要求から更新へ変換する。クライアントから任意basisを受け取らず、nodeローカルのトークンから復元する。
- getとget_conflictsはReadを検査し、非公開kind競合を削除。管理inspectだけが削除済みContentと管理トークンを返す。
- 同じトランザクションで認証・失効・期限・変更前後Writeを検査。一般アプリの全Read/Writeを管理権限と同一視しない。
- 実DBで古いトークンの再利用・再起動後の再送・request_id衝突・Writeのみ・Read縮小・失効・期限・kind Filter内外・非公開kind競合・削除競合/復元を確認。
- 解決トークンの候補外IDを拒否し、解決時に未観測だった編集を保持する。
- request_resultsへのSQL失敗で、更新・結果用トークン・ログがまとめてrollbackすることを検証。
- 2つのDBでContentを同期してもcredential・トークン・再送結果は移らず、古いローカルトークンによる保存が並行枝になることを確認。
- schema v1からv2への移行で既存ログとnode/log epochを保持。
- レビューで発見した修正: Writeのみだった保存結果に、Readを後で付与した際の再送で過去本文が追加されないよう、保存時の返却可否を永続化した。回帰テストあり。
- この段階の検証結果: workspaceの44テスト、fmt、Clippy（警告をエラー扱い）、独立Python record fixtureの照合が通過。
- 次段階: HTTP/CLIホスト、OS保護bootstrap、Peer本人確認と参加資格、実プロセス間同期。OAuth一般アプリ連携はまだ未完成であることをcapabilitiesと文書で明示する必要がある。

## レビュー: HTTPS Peerと直接承認（schema v3）

- protocolにPeer要求・応答・ページのcodecを追加。recordの元bytes、u64 seq、固定highとepochを保持。Statusにrecord版と上限を含め、不一致は停止する。独立Python fixtureあり。
- nativeはRustls/Hyper/Tokioを使用。相互TLSの検証後もleaf DERを登録Node IDに照合。cluster、退役、登録鍵をnodeのトランザクション内で再確認する。
- schema v3はpeersの証明書・endpoint・退役tombstoneを追加。承認にはManageNodesが必要。退役済みID、同IDへの別証明書、同証明書の別ID利用を拒否する。
- 実TCP/TLSのテストで並行編集・削除競合・双方向収束・ACK喪失再送・DB再openを確認。未承認鍵、偽cluster/Node ID、偽サーバー証明書を拒否し、既存TLS接続上でも退役後は403となる。
- レビュー修正: 相手発の同時転送が固定highより先にカーソルを進めた場合は、現在highを確認して追従する。ラウンドを延長せず、同epochのログ巻戻りとは区別する。同時開始テストあり。
- レビュー修正: 不正更新のupdate IDと理由をRejected応答に含める。失敗したページをrollbackした後に応答へ変換する。正常prefixを含む不正ページでもログとカーソルが残らないことを実TLSで確認。
- 接続数・本文・ヘッダー・時間に上限を設け、DB処理は同期callbackをblocking workerで実行。ネットワーク待機中はDBロックを保持しない。停止時は接続taskを回収する。
- サンドボックスはlocalhostのbindを拒否するため、実通信テストは許可された実行環境で実施。外部ホストへのテスト接続は行っていない。
- 未完了: daemon/CLI、OS保護ファイル、独立2プロセスでの再起動・再接続、周期同期、第三ノードへの管理証明伝播、DOの実環境保存境界。目標は継続。

## レビュー: native App HTTPS入口とローカルSDK

- `harmion-client`はprotocolと所有者限定ファイル操作にのみ依存し、nodeやDBへ依存しない。loopback以外・port 0・不正discoveryを拒否し、TLS検証と登録leafの一致を確認してから資格情報を送る。リダイレクトや平文fallbackはない。
- Appホストはloopback限定。HostとAuthorizationの重複を拒否し、Origin付き要求・queryを拒否。接続数・本文・ヘッダー・時間・返却bytesに上限を設ける。ブラウザOAuthは未対応とcapabilitiesで明示。
- 返却JSONはprotocolの専用view変換で作り、内部basis/rootを公開しない。認可とkind競合の秘匿はnodeの既存use caseを通す。管理操作も同じApp入口からManageApps/ManageNodesを検査する。
- Unix所有者限定directory/file、bounded read、symlink/hardlink拒否、atomic replace、daemon用排他lockを実装。二重lock・不正mode・リンク経由アクセスの拒否をテストした。同一OSユーザーからの敵対的なdirectory差替えを隔離する機構ではない。
- 実HTTPSで作成・取得・再送結果一致・request_id衝突・誤credential・誤証明書・Readのみ・管理操作拒否・失効を確認。workspace全57テストとClippyが通過。
- 未完了: daemon初期化・起動・秘密ファイルの実運用への接続、CLIコマンド、周期同期、独立2プロセス検証。これらの完了前に目標達成とは扱わない。HTTP異常ヘッダー、巨大返却、JSON数値境界の追加レビューも残る。

## レビュー: daemon・CLIと独立2プロセス

- 初期化は既存directoryを拒否し、node.jsonを最後に保存する。再起動で同じID・鍵・epochを再利用。DBと既存sidecarを検査し、二重起動はファイルlockで拒否する。
- AppとPeerの独立listenerを起動。実ポートを保存し、停止時に接続taskを回収する。周期同期はbackoff/jitter付き。レビューで発見した改善として、同期workerの予期せぬ終了もdaemonが検知して停止するようにした。
- CLIはSDKのみを使い、DBを直接開かない。保存要求を所有者限定ファイルへfsyncしてから送信し、saveコマンドで同じbytesを再送できる。
- `design_checks/two_process_sync.py`で実バイナリ2つを起動。相互承認、分断編集、解決時に未観測の編集、削除競合とkeep、SIGKILL後の元receipt再送、再起動後の鍵一致、有限echo、手動呼出しなしの周期同期、退役と再承認拒否、秘密ファイルmodeを確認した。
- 実行手順はnative-quickstart.md。最終監査としてHTTP異常入力とJSON境界の確認、変更後の全検証、DO保存境界の未検証事項の整理が残る。

## レビュー: 通信と同期ラウンドの分離（進行中）

- `net-native::sync`へ固定highの双方向ラウンドを移し、`PeerTransport`の要求・応答portを導入。HTTPSはそのadapterとなり、同期ラウンド自体はTLS型・socket・HTTP framingを参照しない。
- transportは承認済みbindingへの本人確認、応答のcluster/node照合、通信の上限・期限を担当する。ラウンドがcursorとページの整合性を検査し、nodeがtransactionを所有する。
- pull応答を保存するためにTLS受信要求を偽装する処理を除去。`membership::receive_response`は通信に使った承認情報と現在の有効な登録をwrite transaction内で比較し、通信中の退役・変更を拒否する。
- socket/TLSなしのadapterでも同じラウンドが双方向転送・有限echoを完了するテストを追加。応答待機中に退役させ、ログとcursorが書かれないことも確認。
- まだ完了していない境界: `Peer`の本人確認情報と登録DTOはTLS証明書を前提にしている。次は鍵の種類と接続先の区別、iroh Endpoint IDの承認・永続化、iroh adapter、daemonへの接続、IP変動/relay経路での再接続検証を実装する。trait追加だけで通信抽象化完成とは扱わない。

## レビュー: irohの鍵・接続とschema v4

- PeerKeyをTLS証明書/iroh公開鍵で型分け。接続先文字列はHTTPSのIP:portまたはirohの任意relay hintで、irohでは空なら探索を使う。Node IDは別のまま保持する。
- SQLite v3→v4で既存証明書・endpoint・退役状態を保持。鍵種別付きの一意性を検査する。再起動後のiroh鍵保持、別鍵への差替え拒否、退役後の再承認拒否をテスト。
- iroh 1.4.1のQUIC stream adapterで共通Peer codec/同期ラウンドを使用。remote_idは接続handshakeから取得し、要求内Node ID/clusterと承認済み鍵を毎回照合する。streamサイズ・接続数・時間を制限。
- 実QUICで127.0.0.1から127.0.0.2へ相手を再bindし、MemoryLookupの探索情報だけを更新。同じEndpoint ID、Node ID、DB、cursorで再承認なしに追加更新を取得。偽Node IDを拒否。これは公開探索/NATの検証ではない。
- daemonは新規初期化でirohを標準選択し、iroh.secretを所有者限定保存。Appの手動sync・周期syncはNativeTransport経由で適切なadapterを選ぶ。リレーは標準/指定URLを選択可能。指定リレーでも公開探索はn0依存であることを実行手順に明記。
- localhostに公式iroh-relayを起動し、両endpointの直接IP transportを無効化。指定relayのみを通じたページ同期が成功することを確認。公開リレー・公開探索には接続しないテスト。
- `--discovery none`と明示relay URLの構成を追加。`two_process_sync.py --iroh`で独立daemon 2つとローカルリレーを起動し、CLI経由の相互承認・分断編集・解決中の追加更新・削除競合・SIGKILL再起動・元receipt再送・有限echo・周期同期・退役が通過。リレーURLとEndpoint IDのみを登録し、IP:portをPeer登録しない。公開探索には接続しない。
- 全62テストとClippyが通過。残る運用上の未検証事項は、公開探索サービス、実インターネットのNAT環境、長時間の切断・リレー自体の障害復旧。ローカルリレー検証をこれらの実環境の保証と扱わない。
- 最終レビュー修正: iroh鍵ファイルが失われた場合に新しい鍵を黙って生成せず、起動を拒否する。初期化が作成した鍵を保ち、Node IDとEndpoint IDの対応を暗黙に変えない。2プロセス検証に鍵紛失時の拒否とファイル非再生成を追加。
