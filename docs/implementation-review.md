# 実装全体レビュー

確認日: 2026-11-18。基準は `e8171cf`。以降の変更と検証結果は台帳の該当項目に記録する。仕様の出典は[specification.md](specification.md)、API契約は[app-api.md](app-api.md)。会話・設計文書の照合は[review.md](review.md)を参照。

指摘を直した場合も項目を残し、状態・修正・検証根拠を更新する。初期仕様の未実装は残作業として扱う。従来の「残す」「維持」は基準時点での判断であり、今後の変更に対する承認を表さない。

## 実装とレビューの進め方

仕様で定まっている動作を単位として実装し、差分・既存動作への影響・検証結果を提示してレビューを受ける。仕様に沿う実装に必要な判断が未確定の場合、判断が必要な箇所で作業を止め、選択肢と影響を所有者に確認する。

- §9.3 のローカル保存後の同期は R16、復帰・ネットワーク変化・iroh relay再接続の同期ヒントはR27に記録する。相手の更新通知は未実装。
- §6.1 の Query は条件と参照評価器を R17、保存層の検索を R18、固定スナップショットと App API・SDK への接続を R19 に記録する。Subscribe は R20 に記録する。
- §6 のnative OAuthはR22、§4.3・§5 の標準解決ツールとローカル管理 Web アプリはR24に記録する。§7 のInterpreterと§9 のCloudflareノードは実装作業が残る。Cloudflareのノード認証と初期管理資格情報は設計が残る。[未解決の設計論点](review.md#4-残る境界注意点新たな確定仕様ではない)

## 確認範囲

domainの更新受理・projection・属性値、protocolのrecord/App/Peer codecとJSON view、nodeの認可・保存・membership・replication、SQLite transaction/schema、HTTPS/iroh adapter、SDK、daemon lifecycle/周期同期、CLI、既存テストと2プロセス検証を読んだ。外部依存の全ソース監査、全OSでの実行、実インターネットのNAT/公開リレー試験は含めない。

## 仕様との対応

| 仕様 | 現状 | 根拠・不足 |
|---|---|---|
| §3 Content、項目別原子保存、固定編集元 | 実装 | `domain::{Graph,ContentSnapshot}`、`node::application::save`。tokenは保存済みbasisを復元する |
| §3.2 複製状況の確認 | 所有者向け実装 | `sync-status` は成功ラウンドで確認したPeerの受領位置、最後の失敗分類と時刻、更新IDの受領確認を返す。現在オンラインとは断定せず、一般アプリの保存結果単位の照会は未公開。R26参照 |
| §4 競合・削除・keep/restore | Coreと標準解決ツールを実装 | `Graph::accept/project`。Webで本文比較・編集、属性/kind選択、三者マージ案、削除と復元を扱う。R24参照 |
| §5 全履歴保持・管理向け取得 | 実装 | 不変recordを保持。管理トークンの候補に対する共通本文版と、削除済みContentの復元候補を取得できる。R24参照 |
| §6 Query/Subscribe | native App API・Rust SDKを実装 | R17〜R20参照。固定結果のページ取得、初回取得、変更の再開、再認可、キャッシュを実装 |
| §6 Read/Write/Manage権限 | 実装 | `node::access/application`。grant/credential/token/request_resultは同期対象外 |
| §6 OAuth | native実装 | R22参照。HTTPS、登録、所有者承認、PKCE、refreshと失効。ブラウザでCAの設定が必要 |
| §6 OS所有者保護 | Unix実装、Linux検証 | `local::ProtectedDir`。Windows/Androidの保護・lifecycleはない。同一OSユーザーを信頼する前提は仕様と一致 |
| §7 Interpreter | 未実装 | 登録、仲介、Call/Provide、Provider接続なし |
| §8 参加・退役の伝播 | 実装・検証済み | 共通の初期信頼証明、署名付きAdmit/Retire/Dismiss、管理ログ・保留・所有者回復。R01参照 |
| §8 古いノードバックアップからの復元 | native 実装 | 新しい Node ID で Content 履歴を復元し、有効なノードから承認を受けて再参加する。R23参照 |
| §9 端末間iroh・通信と意味の分離 | 実装、検証範囲限定 | `sync::PeerTransport`とHTTPS/iroh adapter。ローカルIP変更、リレーのみ、独立2プロセスを検証 |
| §9 同期契機 | 部分実装 | 起動・周期・手動・ローカル保存（R16）に加え、Linuxの復帰推定、実行再開、ネットワーク構成変化、iroh relay接続を同期ヒントにする（R27）。相手の更新通知と全OSの復帰検知は残る |
| §9 Cloudflare | ローカルworkerdまで実装、実配置なし | Worker/DO adapter、Peer v2、管理UI、provisioning、outboundをworkerdで検証（R33〜R51）。実CloudflareのAccess/TLS/secret/課金とDO起動時の全履歴検証は未実施 |
| §10 Export | ネイティブ App API・管理CLI実装 | R21参照。本文、通常値、競合候補、削除記録を固定時点で出力 |

## 指摘・暫定対応の台帳

### R01 — 参加・退役が伝播せず、全組合せで手動承認が必要

- 種別: 仕様との差異／未実装。優先度: 高。状態: 修正済み。
- 修正前: `crates/node/src/membership.rs`の`approve/retire`は`tx.put_peer`のみ。`protocol::PeerCommand`にmembership交換がなかった。
- 影響: AがCを承認してもBはCを認証できない。AがCを退役させてもBはCを受け入れ続ける。現在の2ノードテストの相互approveはこの不足を露呈させない。
- 完了条件: 署名された参加・退役情報の保存と検証、既知の信頼関係からの伝播、受信順序に依存しない退役、承認元退役による連鎖失効の防止、3〜5ノードと再起動の実証。単純なpeers表コピーや「最初に届いた鍵を採用」で埋めない。
- 決定済み: 退役時の確認済み承認を維持し、未確認分は有効なノードで再承認。相互退役は競合として保留し、第三ノードまたは所有者が確定する。
- 修正: canonical HMC署名証明、SQLite schema v5の独立管理ログ・cursor・trust、共通起点のjoin、管理情報を先に交換するPeer経路を実装。元証明を確認できなくなった退役は保留し、有効な第三者が確定／却下する。既存のデータ同期は毎操作で双方の現在の参加資格を確認する。
- 回復: 独立した有効ノードがなければ、新しい回復鍵を各端末の所有者が明示確認する。ローカルな回復trustを、通常の証明転送で自動的に他端末へ昇格させない。
- 設計根拠: [membership-design.md](membership-design.md)、wire・意味の契約は[membership-v1.md](../contracts/membership-v1.md)。データの5ノードテストと管理情報の実プロセステストを分け、手動の全組合せ登録で伝播を装わない。

### R02 — 到達不能なPeerが他のPeerへの同期を待たせる

- 種別: 複数台で露呈する実装制限。優先度: 高。状態: 修正済み。
- 修正前: `apps/daemon/src/main.rs::periodic`が全Peerを逐次awaitし、`net-native::sync::sync_once`がSharedStore全体のsync mutexをラウンド終了まで保持していた。
- 影響: irohの接続失敗1件が最長30秒の待ちを作り、別のオンライン端末も待っていた。
- 修正: 同一Peerの自発ラウンドだけを直列化し、周期同期は最大4ラウンドを並行実行する。DB transactionの短い排他は維持。タスク終了時に接続処理も破棄し、Peerごとの再試行時刻とログ抑制を維持する。
- 検証: `sync_transport::an_unresponsive_peer_does_not_lock_out_another_peer`で永遠に返らないPeerと別Peerの同期を同時に開始し、後者が進むことを確認。`design_checks/multi_process_sync.py --iroh`では5実プロセス中1台停止中にも、別経路で周期同期が進み、停止台の再起動後に追いついた。
- 残る制限: 固定highは更新流入によるラウンド延長を防ぐが、履歴ページ数に比例した全体時間は制限しない。DBを共有するため、重い再projectionの影響はR03として残る。

### R03 — 保存・同期データ処理の全ログ再読込

- 状態は主要経路を修正済み。保存と同期受理は対象Contentの履歴だけを読み、update IDの重複をSQLiteの一意索引で検査する。同期の上限位置とページは受理ログの位置から求め、件数・バイト上限内のレコードだけを読み出す。
- 同じローカルデバッグビルドの測定で、約500件蓄積時の保存中央値は約61 msから約4 ms、差分ゼロの同期は約294 msから約184 msになった。本文は4 KiBで、端末間の実容量や長期運用の上限は未測定。
- R31でschema v7以降の起動時検査もContent単位の逐次検証にした。全レコードの検証は省略せず、参加状態の再構築、単一Content内の長い履歴、schema v6以前からの移行の全Graph保持は残る。全履歴は保持する。

### R04 — JSON返却を全生成してからサイズを制限する

- 保存応答は修正済み。本文と属性を別々に保存して表示が8 MiBを超えると、コミット済みにもかかわらず保存も再送もHTTP 413になっていた。現在はHTTP 200で元の更新IDと `saved: true` を返す。表示が上限を超える場合は `view: null` と `view_error: "response_too_large"` を返す。[App HTTPSテスト](../crates/net-native/tests/app_https.rs)
- 応答生成と失敗した取得のtoken増大はR28で改善した。`app::response`は書込み前に8 MiB上限を検査し、`application::get/read_conflicts`は認可後・token発行前に借用したsnapshotから全応答のサイズを計測する。
- 各recordが1MiB以内でも、属性・競合候補の集合は8MiBを超え得る。現在はその取得を新しいtokenを保存せず拒否する。ただし全snapshotの再構築と、単一Contentの履歴読込みによるメモリ・CPU使用は依然として残る。
- 解消条件: 読出し集合・候補のページング/予算、JSON生成時の上限。保存済みデータを切り捨てて成功応答にする回避は禁止。
- QueryとSubscribeは項目ごとに借用したviewを上限付きの書き込み先へ符号化してサイズを計測し、入る分だけをページ・通知に追加する。get・conflicts・inspectも生成前にサイズを計測するが、一括取得のため大きな1件をページ取得する機能はまだない。

### R05 — 正確な数値と任意objectキーのJSON往復

- 種別: SDKのデータ変形不具合。優先度: 中。状態: 修正済み。
- 根拠: `protocol::app_view`はserde_jsonのarbitrary_precisionを使用し、SDKもserde_json::Valueへparseする。domainが許す任意キーとライブラリの予約表現の衝突を未検証だった。
- 再現: `protocol/tests/app_view.rs`でそのキーを持つobjectが数値123へ変わることを確認。
- 修正: JSON wireは変更せず、`protocol::decode_app_json`がraw tokenのobject/array構造を先に読み、SDKがそれを使用する。キーを拒否・改名せず、正確な数値も維持する。同じ再現テストが通過。

### R06 — HTTPS Peerは固定SocketAddrと自己署名証明書を前提

- 種別: Cloudflare経路の暫定実装。優先度: 中。状態: 残す。
- 根拠: `peer::approve/exchange`はSocketAddr parseと相手leaf固定。`TlsIdentity`は自己署名で`harmion.local`を使用する。
- 影響: 任意HTTPS URL・DNS・Cloudflare入口のnode本人確認を扱っていない。native間の検証経路として使えるが、Workersへそのまま接続できるとは主張しない。
- 解消条件: WorkersホストとHTTPS認証の実装を合わせて検証。証明書検証を無効化する回避は禁止。

### R07 — App APIの実装形式は当初のHTTP/WS・JSON-RPC候補と異なる

- 種別: 明示した設計変更。状態: 維持。
- 根拠: loopback HTTPS、POSTの固定route、保存HRQ binary、明示JSON view。Origin付き要求を拒否する。
- 理由: native管理CLIを先行し、ポート再利用時の資格情報漏洩を防ぐため証明書を固定。architecture/app-apiに差を明記済み。
- 制約はnative App APIに適用する。OAuth認可にはR22の別のHTTPS入口を使う。App APIのブラウザ直接呼び出しとWSは未実装。

### R08 — 管理CLIは本文中心で、一般アプリSDKも薄い

- 種別: 未実装／暫定UX。状態: 残す。
- 根拠: CLIのedit/resolve/restoreはBodyを直接入力する。既存のrestoreは省略した既知属性/kindを消す。R24でCLIの候補選択と共通祖先取得、およびWebでの型を保持する復元を追加した。SDKの個別Contentの取得結果はJSON Value。型付きQuery要求、購読の状態管理とキャッシュはR19・R20で追加。
- 影響: Coreの属性/kind操作が存在しても、標準ツールで全操作できるわけではない。restoreの省略値を保持するようCoreの意味を変えて隠さず、現在の制約をhelp/手順に明記する。

### R09 — エラーの診断情報がAPI境界で失われる

- 種別: 暫定実装。状態: 一部改善、一般のエラー応答は残す。
- R26で所有者向け同期状態に、最後の失敗の限定codeと時刻を保存・公開した。過去の受領確認と失敗を分離する。HTTPエラー応答やSDKの一般エラー型は変更していない。
- 根拠: SDKは非200を`Http(status)`のみへ変換、AppはPeerの拒否理由を`peer_unavailable`へまとめる。irohの不正frame/認証失敗は接続close。
- 影響: 運用時に不正データ、退役、到達不能を区別しにくい。同期記録をACK済みにすることはないが、仕様§9.3の理由付きエラーの公開は不十分。
- 解消条件: 秘密情報を含めない構造化codeとcause、同一エラーのログ抑制、CLI表示を整理する。

### R10 — tokenと再送結果が無期限に増える

- 種別: 寿命契約未確定の暫定保存。状態: 残す。
- 根拠: `mint_token`はgetごとにinsertし、schemaにexpiry/GCがない。request_resultsも削減しない。
- 影響: 長期運用のDB増大。token再利用・request_idの冪等性が仕様なので、任意TTLで黙って消す修正はしない。期限/再取得/再送保証を定めてから削減する。

### R11 — 公開リレー・探索の標準利用と実環境の未検証

- 種別: 運用設定／検証範囲。状態: 明示済み。
- 根拠: `iroh_peer::bind`の標準はN0。自前relayと`--discovery none`あり。テストはloopback IP変更・公式ローカルrelay・公開探索なし。
- 制約: 公開探索や実NAT、relay長期障害の保証はない。設定可能というだけで全経路検証済みとしない。

### R12 — 過去の進捗文書に未完了表記が累積している

- 種別: 文書保守。状態: 修正済み。
- 根拠: `implementation-progress.md`は過去時点の未実装項目を履歴として残し、その下へ完了記録を追加する。`contracts/peer-v1.md`にも独立2プロセス未完了の古い記述がある。
- 対応: 過去の進捗文書に履歴であることと現状台帳へのリンクを明記。Peer契約の独立プロセス検証・周期駆動・鍵導入に関する古い未実装表記を更新した。

### R13 — 退役済み鍵からの後着証拠による保留と可用性

- 種別: 採用した信頼モデルの制約。状態: 明示して残す。
- 根拠: 信頼済みの経路に結び付く未却下の退役証明は、発行元の権限を確定できなくても対象を保留する。未知の自己署名だけでは保留を作らない。
- 影響: 正当な分断中の証明と退役鍵で後から作られた証明を署名・時刻だけでは区別できないため、既知の退役鍵によって手動判断が必要になる可能性がある。保留中は新たな管理承認とデータ同期を止める。可用性妨害まで防ぐ合意プロトコルとは呼ばない。
- 理由: 発行元が退役した際に対象を自動復活させないというユーザー決定を満たすため。管理証拠だけの交換を許し、データアクセスへ昇格させない。元の仕様が未確定だった部分を明示した決定であり、到着順で隠すworkaroundにはしない。

### R14 — 接続先変更の順序付き伝播は未実装

- 種別: 発見・経路管理の不足。状態: 残す。
- 根拠: 参加証明は初回の経路情報を持つが、既存Peerのendpointは受信した過去証明で上書きしない。所有者の再approveではローカルな経路を更新できる。
- 影響: irohのIP変更はEndpoint IDと探索で扱えるが、公開探索なしでリレーURL自体を変更した場合は各接続元の情報更新が必要。旧い証明の再送で新しい経路を巻き戻さない一方、routeの署名付き世代管理・gossipは実装していない。
- 解消条件: 本人確認鍵の変更と分けた経路広告・世代・再送規則を実装し、接続先の更新順序を入れ替えて検証する。

### R15 — 管理要求には保存要求と同じ再送receiptがない

- 種別: 管理APIの未実装契約。状態: 残す。
- 根拠: Content保存はrequest_idと元のreceiptを保持するが、approve/retire/保留の確定・却下はその仕組みを共用していない。Peer経由の署名証明の再送はIDで冪等。
- 影響: 管理CLIの応答を失った場合、再実行で証明が追加されたり、既に解決済みのため拒否されたりする。元の応答を必ず再取得できるとは主張しない。membersで反映状態を確認する必要がある。
- 解消条件: 管理要求のrequest_id・固定観測範囲・元receiptの契約を定めて永続化する。署名証明のネットワーク再送と、所有者操作の再実行を混同しない。

### R16 — ローカル保存が次の周期照合まで同期を起こさない

- 種別は仕様 §9.3 の同期契機の不足。状態は実装済み。セルフレビューで見つけた割り当ての問題も修正済み。
- 修正前は、daemonの自動同期が保存確定を受け取る経路を持たなかった。
- AppのContent保存で新規更新がコミットされた後、DB workerからdaemonの自動同期を起こす。同一要求の再送は元のreceiptを返し、認可・保存に失敗した場合も同期シグナルは発行しない。
- 複数のシグナルをまとめ、同期中の追加保存はPeerごとの次回ラウンド候補にする。到達不能なPeerのbackoff、同一Peerの直列化、`--sync-seconds 0`の手動同期モードは従来どおり。保存の応答はローカル永続化で返す。
- 確認箇所は [daemonの自動同期](../apps/daemon/src/main.rs)、[Appの保存経路](../crates/net-native/src/app.rs)、[保存トランザクション](../crates/node/src/application.rs)。
- App HTTPSテストで新規保存の通知と、同一要求の再送・request ID衝突・書込権限不足で通知が発生しないことを検証した。[App HTTPSテスト](../crates/net-native/tests/app_https.rs)
- 2つの実daemonを使い、起動ラウンドで最初のContentが届いた後、自動同期周期が120秒の発信元で別のContentを保存した。受信側の自発同期を無効にした構成で、HTTPS・irohとも15秒以内に2件目が届いた。[実プロセス検証](../design_checks/two_process_sync.py)
- 5つの実daemonを使う既存検証もHTTPS・irohの両方で通過した。分断中の編集、中継、後着競合、クラッシュ再起動、1台停止中の自動同期と停止台の追従を確認した。[5プロセス検証](../design_checks/multi_process_sync.py)
- ワークスペースのRustテスト78件、Clippy、fmt、codec fixture、Pythonの意味モデル492順列が通過した。
- セルフレビューで、保存が続き同期候補が5件以上あると、Node ID順で毎回先頭から4枠を割り当てるため、後方のPeerが実行されない場合を確認した。退役済みPeerの経路も管理証明の交換用に保持するため、有効ノードが5台以内のクラスタでも候補が5件以上になり得る。
- 修正後は実行可能なPeerを待ち行列に追加し、空いた枠を待機順で割り当てる。daemonの回帰テストで、4枠が埋まった状態で保存が続いても5番目のPeerが次の枠を取得すること、同期失敗後は保存があっても再試行時刻まで待つことを確認した。[スケジューラとテスト](../apps/daemon/src/main.rs)
- 割り当て修正後のRustテスト80件、Clippy、fmt、HTTPS・irohの2プロセス検証が通過した。

### R17 — 型付きQuery条件の評価

- 対象は仕様 §6.1 の条件評価と §6.2 のRead Filter。状態は評価器を実装済み・セルフレビュー済み。
- protocolに型付き条件を定義し、nodeで通常表示のkindと属性を評価する。数値は正確な10進値で比較する。文字列の比較順、型が異なる場合、不在と`null`、空の論理条件と配列条件を契約に記録した。[条件の評価](app-api.md#条件の評価)
- getとQuery評価器で読み取り可否の判定を共有する。削除済みContentを除き、現在のRead Filterを条件とは独立に適用する。削除競合では生存側の通常値で判定する。
- テストでは数値の指数境界、等値と大小比較、配列の順序と重複、不在と`null`、kindの通常値による認可、削除と編集の競合を確認した。[評価器のテスト](../crates/node/tests/query.rs)
- ワークスペースのRustテスト83件、Clippy、fmtが通過した。
- Contentの最終編集時刻を通常表示の項目版から求め、UTCマイクロ秒の大小比較を条件に追加した。選ばれた属性のunsetとkindの非所属も含む。通常表示から外れた版や削除・keepの時刻は集計対象外。domain・nodeのテストとワークスペース全ターゲットのClippy、fmtが通過した。[条件の評価](app-api.md#条件の評価)
- セルフレビューで、時計のずれにより後継版の時刻が前の版より古い場合に、最終編集時刻が戻ることを確認し、API契約に追記した。時刻条件はRead Filterと削除状態の判定を通過した後に評価する。ワークスペースのRustテスト84件が通過した。
- 保存アダプタでの検索はR18・R19、Rustの単一Snapshot判定を使う購読の変更処理はR20に記録する。[保存アダプタの設計](architecture.md)

### R18 — native SQLiteでのQuery実行と検索索引

- SQLite schema v6は通常表示からkind・属性・配列要素・最終編集時刻の索引を生成する。旧DBは保存済み更新を検証し、同一トランザクションで索引を再構築して移行する。
- ローカル保存と同期受信は、受理ログと同じトランザクションで変更されたContentの索引を更新する。削除確定で検索対象から外し、復元で戻す。同期受信で同一Contentの更新が同じページに複数ある場合は、そのページの最終状態を索引に反映する。
- SQLiteの保存アダプタが型付き条件とRead Filterをパラメータ付きSQLに変換し、絞り込み後にContent ID順で件数を制限する。数値の大小比較は正確な10進数と等価なバイト順を使う。[SQLiteでの実行](../crates/store-sqlite/src/query.rs)
- 参照評価器との一致、Read Filter適用後の件数制限、既存DB移行、受信した削除と復元、ローカル保存と同期受信それぞれのロールバックをテストする。検索自体は受理ログを読み込まない。更新受理時の全ログ再読込は R03 に残る。
- ワークスペースのRustテスト87件、Clippy、fmtが通過した。
- この保存操作の結果は同一トランザクション内の現在値に基づく。複数のAPI呼び出しをまたぐ固定結果の公開経路は R19 に記録する。

### R19 — 固定Queryのページ取得

- schema v7で現在の編集元とContentごとの受理ログを索引化する。API開始時の条件とRead Filterで選んだID・編集元をSQLiteに固定し、後続ページでは候補Contentのログだけを読む。
- 不透明カーソルをアプリと固定結果に結び付ける。版の再構築と編集トークンの発行は、最新の認可と固定時の可視状態を確認するトランザクション内で行う。期限、同時に保持する結果の数、版参照の総量、応答のバイト数を制限する。
- HQU v1要求、App HTTPS、Rust SDKのページ取得を接続した。固定版の再構築、継続時の再認可、編集元の保持、別アプリへのカーソル流用拒否、期限経過、再起動後の継続をテストする。[ネイティブのQuery](app-api.md#ネイティブの-query)
- ワークスペースのRustテスト89件、Clippy、fmtが通過した。
- セルフレビューで、多数のRead kindをSQLの個別パラメータに展開するとドライバの上限に当たることを確認した。開始時の認可には保存済みのgrantをSQLで結合し、33,001件のkindを含むgrantで回帰テストした。

### R20 — 変更購読と再開

- schema v8で、Contentの検索索引と同じトランザクションに変更位置を記録する。ログを4096件に制限し、認可変更にはアプリごとの世代を設ける。初回の固定結果と変更履歴の基準点を同時に確定する。
- App HTTPSで開始・再開・終了を実装する。待機中はローカル保存、Peerからの受信、grant変更で起こす。通知には購読内の順序を付け、再開時は最新の可視状態で作り直す。見えていた項目が権限外になった場合はIDだけを返す。
- SDKは初回完了までキャッシュを仮状態にし、通知を適用してからカーソルを更新する。セルフレビューで、初回中に非同期要求がキャンセルされた場合も仮状態を破棄するよう修正した。古い通知による巻き戻しを回帰テストで確認する。
- 期限・再起動・grantの変更・変更履歴の保持超過では `reset_required`。資格情報を失効させた場合は HTTP 401。Cloudflare側は別途保存・ホストアダプタを実装する。[ネイティブのSubscribe](app-api.md#ネイティブの-subscribe)
- 検証では、初回取得中の複数編集の集約、応答再取得、kind変更でのIDのみの削除通知、同じ値に戻したgrant変更、保持範囲超過、初回確定までのSDKの仮状態、順序の逆転、Peerのpushとpullによる待機解除、ノード再起動後のresetを確認した。[SQLiteの購読テスト](../crates/store-sqlite/tests/application.rs) [HTTPSのPeer同期テスト](../crates/net-native/tests/https_sync.rs)
- 最終確認でRustテスト94件、Clippy、fmt、HTTPS・irohの実daemonの2プロセス同期検証が通過した。

### R21 — 所有者向けExport

- CLIは新規の所有者用ディレクトリに本文とメタデータを書き、転送終端を確認した後にマニフェストを公開する。対象範囲、各レコードの意味と、出力の読み方を [Export v1](../contracts/export-v1.md) に記録した。
- 起点の受理ログ位置を固定し、Contentごとに索引から履歴を読み出す。取得単位で管理者権限を再確認し、DBの排他中にネットワークの読み手を待たない。
- 保存された削除記録は復元済みの操作も含む。通常表示から外れた解決済み本文、認可・資格情報、同期の運用状態は出力範囲外。
- 実daemon・CLIで、合計8 MiBを超える転送、UTF-8本文のバイト一致、未解決候補と削除記録、所有者用の権限、既存出力先の保持を確認した。Rustテストでは出力開始後の編集の隔離、権限失効、終端の欠落・件数不一致、属性の精度を検証した。[実プロセス検証](../design_checks/export_process.py)

### R22 — native OAuth

- 所有者が登録したnative public clientをAuthorization Code＋PKCE S256で認可する。既存管理資格情報で発行した単回リンクにブラウザの承認を結び付け、OAuthではRead・Writeを付与する。
- OAuthは別のloopback HTTPSリスナーで提供し、初期状態では無効。所有者は専用CAをブラウザとトークン交換クライアントに登録する。CAの署名鍵はサーバー証明書の発行後に破棄する。[有効化と更新の手順](native-quickstart.md#oauthを有効にする)
- SQLite schema v9にノードローカルの登録・認可・資格情報を格納する。コードとrefreshの単回消費、再利用時の系列失効、grantの再承認をトランザクションで処理する。SDKは取得済みaccess tokenで既存App APIへ接続できる。
- 実daemon・CLIで、未信頼CAの拒否、正しいCAでのHTTPS、承認と拒否、不正Origin・redirect・PKCEの拒否、kind権限、再起動後のrefresh、再利用・縮小・管理者失効を確認した。RustのSQLiteテストでIPv6 callback、期限、旧schemaからの移行を確認した。[実プロセス検証](../design_checks/oauth_process.py)
- 一時Firefoxプロファイルに専用CAを登録し、HTTPSの認可画面を表示して承認し、loopback callbackで受け取ったコードをトークンに交換できた。ほかのブラウザとOAuthクライアントライブラリの互換性は未検証。

### R23 — 古いバックアップ復元時のログ位置再利用

- 古いノードディレクトリを元の Node ID のまま起動すると、新しい保存が相手の受領済みログ位置に入り、同期成功後も相手へ届かないことを2ノードで確認した。
- 仕様§8に従い、`harmiond restore BACKUP_DIR NEW_DIR` はバックアップの受理済み履歴を読み、新しい Node ID、ログ世代、鍵、資格情報で復元する。元のノードディレクトリの直接起動にはこの復旧処理が適用されない。[復元手順](native-quickstart.md#古いノードバックアップからの復元)
- 実daemonで、有効なノードから再承認を受け、バックアップにだけある更新と既存ノードにだけある更新を双方で取得し、復元先の新規保存も同期できることを確認した。[実プロセス検証](../design_checks/restore_process.py)

### R24 — 標準解決ツールとローカル管理 Web アプリ

- 属性・kind・本文の候補を版 ID で指定し、固定トークンに含まれる候補と明示した置換対象を照合する。復元では本文と選択した属性・kind を指定する。正確な属性値はノードが固定候補から取得する。
- 共通本文版を取得できるのは所有者の管理トークンで指定した本文候補に限る。曖昧な共通本文版は一覧を返す。SDKが行単位の三者マージ案を生成し、所有者が内容と置換対象を確認して保存する。
- `harmion-admin` は既存SDKでHarmionに接続する独立アプリ。ブラウザは専用のloopback HTTPSで単回リンクを短命なセッションへ交換する。Host・Originとセッションを確認し、外部オリジンと一般アプリからの資格情報なしの操作を拒否する。 [起動方法](native-quickstart.md#管理-web-アプリ)
- 固定候補の選択、共通本文版の曖昧さ、権限外の拒否をRustテストで確認した。実daemonと管理Webプロセスでは、単回リンク、別オリジンの拒否、マージ案生成時の無更新、明示保存、後着版の保持、正確な数値を含む復元、参加承認、OAuth登録・失効とExportを確認した。一時Firefoxプロファイルでの起動とマージ保存も確認した。 [実プロセス検証](../design_checks/admin_process.py)

### R25 — 管理 UI の画面構成・Peer とアプリの接続

- 管理 UI を Svelte・TypeScript・Vite に移行し、ダークテーマを標準とした。コンテンツ閲覧と競合編集、概要・Peer・アプリ・Export を分け、同一セッション中の画面移動では編集内容を保持する。保存結果が不明なときは同一 request ID の再試行を画面上部に残す。
- 所有者専用 Metrics API で Content 数・更新数・レコード量・有効登録数を集計する。Peer の Active は参加状態として表示し、到達性については手動同期の結果と時刻を表示する。
- Peer は QR の生成・画像読み取り・カメラ読み取りと接続コードの貼り付けに対応する。登録前に Node ID・Cluster ID・公開鍵を表示し、2分以内の単回確認値を使って承認する。
- アプリは登録・認可を1行のコードから開始する。要求のプレビューに保存済みのアプリ名・callback・要求権限を表示し、ノードの認可画面へ引き継ぐ。grant の変更も JSON 入力からフィルターの選択へ移行した。
- 実プロセス検証で、プレビューだけでは Peer を登録しないこと、確認値の再利用拒否、クラスタ違いの拒否、1行のアプリ要求と実際の権限の一致を確認した。Metrics の一般アプリからの拒否も回帰テストで確認した。
- Firefox の一時プロファイルで、ダークテーマ、コンテンツと競合解決の切替、編集保持、マージ保存、QR画像の読み取りから確認後のPeer登録、1行のアプリ登録・認可要求、モバイル幅の表示を確認した。カメラ実機の検証は残る。Rustの103テストとSvelteの型・整形チェック、Clippyが通過した。

### R26 — 所有者向け同期状態と保存更新の受領確認

- 2026-11-20。App HTTPS の `sync-status`、型付きRust SDK、CLIの `sync-status [--update UPDATE_ID]` を追加した。全管理権限に限定し、一般アプリへのPeer情報公開や複製台数のポリシーは追加しない。[API契約](app-api.md#所有者向け同期状態更新の受領確認)
- SQLite schema v10で、Peerごとに最後の成功ラウンドで観測した受領位置とepoch・確認時刻、最新の試行失敗を保持する。旧DB移行は観測なしから始め、バックアップを新しい Node ID で復元した先には転記しない。
- 更新IDを自分の受理ログ位置へ照合し、相手が認証済みCursor／ACKでその位置まで永続受領した場合に確認済みとする。未確認は不在の証明ではない。未確認件数は欠番を考慮して実レコード数を数える。受信しただけのPeer状態から送信完了を推測しない。
- 自発同期の最新の失敗は限定codeと時刻で返し、過去の受領確認を消さない。次の成功で失敗表示を消す。外部のエラー文・接続先・本文・資格情報は公開しない。取得自体はネットワーク接続を起こさない。
- 初版は完了した固定highラウンドだけを観測として保存する。途中失敗の一部ACKは新しい成功観測にしないため、確認は保守的。観測保存要求前のキャンセルや未コミットのクラッシュ・保存失敗では以前の観測が残る。保存要求後は呼出し元のキャンセル後もDB workerが保存し得る。現在のオンライン状態、第三者への伝播、将来のバックアップを保証しない。
- 独立レビューで、非同期の同期タスクをキャンセルしても `spawn_blocking` の観測保存が続き、同一Peerの次の試行を古い結果で上書きし得ることを指摘された。観測の保存完了まで所有されたラウンドロックをDB workerに保持させ、呼出し元の生存期間から分離した。
- SQLiteテストで再open、旧schema移行、Contentのみのバックアップ復元、更新追加後の未確認、未知ID、失効・期限切れ・権限縮小、欠番・epoch不一致、rollbackと退役を検証した。ネットワークテストでACK喪失後のCursor再確認、不正Cursor、同期中の退役、失敗後の過去確認保持と再試行、固定high後の受信更新を確認した。[SQLiteテスト](../crates/store-sqlite/tests/sync_status.rs)・[転送テスト](../crates/net-native/tests/sync_transport.rs)
- 呼出し元を観測保存中にキャンセルする回帰テストで、成功・失敗の両経路についてロックがDB workerに残り、その後の再試行結果を古い保存が上書きしないことを確認した。[キャンセルの回帰テスト](../crates/net-native/src/sync/cancellation_tests.rs)
- ワークスペース全Rustテスト、Clippy、fmt、Python意味モデル492順列、record fixtureが通過した。実daemonの2プロセス検証ではHTTPS・irohの両方で未観測・更新単位の受領確認・オフライン保存・失敗後の過去確認保持・クラッシュ再起動後の保持を確認した。既存の5プロセス同期検証も両経路で通過し、実プロセスの新しいNode IDへのバックアップ復元も通過した。

### R27 — 復帰・ネットワーク変化・relay接続時の同期

- 自動同期が有効なdaemonに、復帰・ネットワーク変化・relay接続の監視を追加した。LinuxのOS復帰は安全なrustix APIでkernelのboottimeとmonotonicの差の増加を検知する。実行間隔の長い空白は別の `RuntimeResumed` ヒントとし、時刻変更やプロセス一時停止をkernel suspendと混同しない。
- netwatchのOS由来のアドレス・経路変化と5秒ごとのローカル状態再確認、irohの新しいhome relay接続を再試行の合図にする。初期状態を復帰と断定せず、インターネットや特定Peerの到達性を保証しない。監視の初期化失敗は30秒後に再試行し、周期同期を維持する。
- スケジューラは2秒の整定期間と30秒の再試行波の間隔を設け、ヒントを集約する。backoffを1回だけ迂回し、失敗履歴は維持する。同期中のヒントは次のラウンドに残し、通常の完了時刻で消さない。同時4枠と待機順を維持し、`--sync-seconds 0` は監視も自発同期も起動しない。
- 検知器の模擬clockテストは最初のsuspend、通常の経過、時刻に依存しない判定、欠損・逆行・遅い採取、runtimeの空白を確認する。スケジューラのテストは長いbackoffの迂回、inflight中の保持、連続ヒントの上限制御、待機順を確認する。実relayの停止・同一アドレスへの再起動による通知と監視終了も統合テストする。[監視実装](../crates/net-native/src/recovery.rs)・[relayテスト](../crates/net-native/tests/recovery.rs)
- ワークスペースのRustテスト・Clippyが通過した。HTTPS・irohの実daemonで、起動時の失敗により長いbackoffが残った端末を `SIGSTOP/SIGCONT` で一時停止・再開し、120秒の周期を待たずに相手の新しい更新を取得することを確認した。`sync-seconds 0` の端末は再開しても自発同期しないことも確認した。[実プロセス検証](../design_checks/recovery_process.py)
- 全OSのサスペンド通知、同一構成のまま起きる上流障害の完全な検知、相手の更新通知は追加していない。実kernelのサスペンドやホストの実ネットワーク変更はこの環境では実施していない。実プロセス一時停止はkernel suspendの検証と区別する。

### R28 — 大きな一括取得の符号化と失敗時のtoken増大

- get・conflicts・inspectは、認可後のsnapshotを借用して返すJSON全体のサイズを計測する。属性値・field・候補・版IDを逐次符号化し、上限超過で停止する。候補のvectorやJSON treeを作ってから計測せず、token発行と出力のcloneより前に拒否する。
- 超過時は従来どおり HTTP 413 / `response_too_large`。データや候補を切り捨てて成功にはしない。全管理権限のないアプリにkind候補を公開せず、認可・不存在・削除判定をサイズエラーより先に行う。
- HTTPSの最終応答は、各writeで残り予算を検査するwriterで符号化する。保存結果は従来どおりコミット済みreceiptを返し、viewのみが大きい場合も `saved:true` と元の更新IDを取得できる。
- protocolテストは完全なread/conflict envelope、NバイトとN−1バイトの境界、escape・UTF-8・正確な数値・任意objectキー、kindの可視性、超過時の反復停止を確認する。SQLiteは繰り返した大きな取得の拒否でtoken・basis・receipt・受理ログが増えず、再open後も同じこと、認可判定・tokenのpurposeと固定basis・再利用・保存の再送結果が変わらないことを確認する。[サイズテスト](../crates/protocol/tests/app_view_sizes.rs)・[SQLiteの大容量取得テスト](../crates/store-sqlite/tests/large_reads.rs)
- 独立の読取りレビューで認可・wire形式・固定basis・保存receiptへの新しい問題がないことを確認し、全Rustテスト・Clippy・fmt、HTTPS／irohの2・5プロセス同期、意味モデル492順列とrecord fixtureが通過した。
- 全履歴の再projection自体、候補のページ取得、単一の大きな項目のストリーミング、正当な取得で発行されたtokenとreceiptの保持期限は変更していない。R03・R04の残る部分とR10を解消済みとは扱わない。

### R29 — Linuxユーザーサービスと日常運用の手順

- 所有者指定のOS優先順はLinux → Android → Windows。常設レプリカはCloudflare Workersを優先し、Androidは電池・回線制約のある断続接続ノードとする。Interpreterは後回しにする。
- Linux向けに任意のsystemdユーザーサービスtemplateと、release build・所有者限定の配置・明示初期化・ログ・更新・バックアップ・復元の手順を追加した。foreground daemonを `Type=simple` で追跡し、umask・再起動の間隔と回数制限・SIGTERMでの停止を設定する。[unit](../deploy/systemd/harmion@.service)・[運用手順](linux-service.md)
- unitはinit・鍵生成・復元・CA設定を自動実行せず、保存済み設定と所有者限定のノードディレクトリを使う。ログアウト後の常駐を保証せず、lingerや実際の導入は所有者の明示操作とする。古いバックアップを同じNode IDで起動するrollbackは手順に含めない。
- 設定のallowlistと、一時的な実行ファイル・targetだけを使う `systemd-analyze --user verify` による静的検証が通過した。ホストへのインストール、サービスの有効化・起動、実systemd lifecycleの検証は行っていない。[静的検証](../design_checks/linux_service.py)
- Cloudflareの前提確認としてdomain・protocol・nodeの `wasm32-unknown-unknown` 向けcheckが通過した。これはDO保存・同期・認証を実装済みとするものではない。

### R30 — Workers SQLのローカル実ランタイム検証

- NixでNode.js・Pythonを用意し、exact versionとintegrityを固定したnpmのworkerdを使う独立した技術probeを追加した。Harmionの本番Workerや認証の代わりではない。loopbackの空きportと新しい一時保存領域だけを使い、外向きネットワークを許可せず、終了時に全子プロセスを回収する。[再現手順と限界](../design_checks/worker_sql/README.md)
- 実際のSQLite-backed DOで、正常prefixの後・cursor書込みの後に投げた例外によるrollback、同一ID同一bytesの重複、異なるbytesの拒否、TERM・SIGKILL後のidentityとログ・cursor保持を確認した。killは保存flush後であり、commit途中のクラッシュや電源断を保証しない。
- `transactionSync` のcallbackからError objectを普通にreturnするとcommitすることを確認した。RustのErrは通常値として返さず、Rustframeの後でJS例外へ変換するbindingが必要。Rustのpanic/trapや完全なStore適合性をJS probeの結果だけで保証しない。
- i64両端と2^53を超える値は、文字列parameterからSQL INTEGERへcastし、TEXTとして取得すると正確に往復した。BLOBもbyte一致を確認した。通常のJS Numberでは `9007199254740993` が `9007199254740992` になるため、DO adapterでの数値変換は無条件にNumberを経由させない。
- 別のstandalone Rust/Wasm spikeでは、非`'static`の借用を含むFnOnceを `transactionSync` に渡し、通常のRust Errで元のエラー・cleanupを保ちながらSQLとcursorをrollbackすること、成功時のcommitとu64文字列parameterのbyte一致を実ランタイムで確認した。Harmionの全Transaction portや本番のDO adapterはまだ実装していない。
- 追加したJS shimでは、通常のRust Errだけをtagged resultに戻し、Wasm RuntimeErrorはRustのcatch importへ戻さず外へ伝播する。同じgeneric FnOnce wrapperでのpanicがSQLをrollbackし、直後のRustカウンタが進まず、poison後の全generated functionが呼出し前に拒否されることを実測した。callback成功後・実transaction内のhost例外もrollbackして誤成功を返さない。
- panic対象のexportはJsValue ABIとし、generated JSのfinally/freeによるpoisoned Wasmへの再突入を避け、codegen形も検査する。再開は新しいworkerdプロセスで確認した。実DO isolateの自動破棄・再生成、一般のunwind・OOM・全Store port・実Cloudflareの可用性は未検証であり、production adapter完成とは扱わない。
- 所有者はCloudflare初版を基本的に同期専用とし、将来の遠隔管理を追加できる構造、HTTPS＋ノード署名、所有者による初期鍵・クラスタ情報の導入という方向を確認した。具体的な契約は[Cloudflare設計案](cloudflare-design.md)で整理し、Tailnet／Accessは参加認証と区別する。

### R31 — 起動時の履歴検査をContent単位にする

- schema v7以降のDBは、全履歴のGraphを同時保持せず、Content・受理位置の順でレコードを逐次decodeし、一つのContentだけのGraphで完全性を検査する。各行の元bytesをまとめたVecや、全Content IDの集合も作らない。元のログは省略・削減しない。[起動時検証](../crates/store-sqlite/src/startup.rs)
- Content別索引を正本として信頼しない。正本accept_logの全行がちょうど一つの索引行に対応すること、孤立行がないこと、全体のupdate ID／位置が重複しないこと、各レコードのcanonical bytes・digest・ID・Content割当・正の位置・依存関係を検査する。破損は修復・無視せず起動失敗とする。
- schema v6以前は索引構築の移行に全Graphが必要なため従来経路を残す。参加状態の検証や後続migrationは維持し、schema・API・保存の意味は変更しない。
- 複数Contentの交互履歴、欠番、競合、削除、旧schema移行、索引の欠落・孤立・誤割当、ID・位置の重複、bytes・digest不正、別Contentへの不正参照、root衝突等をテストした。実SQLiteのquery planは索引のcovering scanと正本のprimary-key lookupを使い、元bytes全体の一時sortを行わないことを確認した。[起動回帰テスト](../crates/store-sqlite/tests/startup.rs)
- 独立の読取りレビューで、依存関係・全体のID重複・正本の検証漏れや旧移行の退行がないことを確認した。全Rustテスト・Clippy・fmt、実daemonのバックアップ復元とHTTPS／irohの復帰同期検証も通過した。
- 単一Contentの大きな履歴、参加証明全体の再構築、SQLite自体のメモリ、search projection等の全派生索引の破損検査を解消したとは扱わない。起動時間や総RAMに固定の上限を保証する変更ではない。

### R32 — 実験的な署名HTTPS transport envelope codecのみ

- 署名HTTPS draftのbounded canonical frame、型付きmessage/error、用途別signing bytesとframe hash、canonical HTTPS DNS originの構文検査をprotocolに追加した。標準URL parserを使い、数値hostのIP別表現・userinfo・非HTTPS・非canonical originを拒否する。[codec](../crates/protocol/src/https_peer.rs)
- 署名は不透明な64 bytes。暗号検証・信頼・freshness・challenge消費・HMC v2・ネットワーク入口を実装したわけではない。現行nativeのcapabilitiesやendpointへ接続せず、Cloudflareレプリカ対応済みとは扱わない。
- 独立Python generatorによる18 fixtureと、7件のfocused testで5種類のframeとresponse-error、元HPQ/HPS bytes、制限・不正入力・identity/class・signing input/hashを確認した。既存record・membership・peer fixtureは維持する。[テスト](../crates/protocol/tests/https_peer.rs)・[fixture照合](../design_checks/https_peer_fixture.py)
- 所有者のquota節約指示により、この小単位を区切りとして新たなCloudflareレプリカ・Android・Windows実装には進まない。実装済み／未実装と次回の手順は[引継ぎ文書](implementation-handoff.md)へまとめる。

### R33 — 既知署名identityの永続pin

- 署名HTTPSの本人確認を現在の参加grantから分離した。`(node, signing_key, binding)`をprovenance付きで永続化し、退役・競合・未確認になっても保持する。pinは書き換えず、同じ鍵または同じbindingを別Node IDへ再利用しない。SQLite schema v11で追加し、`peers`のbinding種別に署名HTTPS用の種別2を予約した。[pin実装](../crates/node/src/identity.rs)・[保存](../crates/store-sqlite/src/lib.rs)
- 挿入は明示的な所有者操作（approve・join・回復trust）と、受理transaction内で有効だったadmissionだけに限る。`apply`は同じtransactionでActive参加者のpinを挿入し、失敗すれば参加受理ごとrollbackする。受信しただけの履歴やcut外の主張からはpinを作らない。[参加状態](../crates/node/src/membership_state.rs)
- 旧DB移行は、現に有効なActive参加者の正確な鍵・bindingと、trust root／回復rootだけをseedする。署名鍵が検証できない旧peer行や退役済みの古いpeerはunknownのまま残し、v2の認証には使わない。nativeの既存経路は引き続き動作する。有効なActive参加者にpinが無いDBは起動時にfail closedする。
- 受理時に無効なcut外Admitを退役issuerが作っても、犠牲者のpinと有効な参加状態を変えないことを実SQLiteで確認した。poison鍵は認証に失敗し、pin済み鍵のevidence交換は成功する。未知・target-only・鍵再利用・pin上書き・移行前peerを拒否するテストも追加した。[回帰テスト](../crates/store-sqlite/tests/identity_pins.rs)

### R34 — HMC v2 SignedHttps bindingと登録形式v4

- HMCにversion 2とtag 2 `SignedHttps([u8;32])`を追加した。bindingの鍵は`Member.signing_key`と一致することをencode/decodeで強制する。v1 TLS DER・irohの元bytesとversion 1のRetire/Dismissはバイト同一のまま。v2のRetire/Dismissは導入せず、bindingを持たない操作はv1で出す。読み手はv1とv2の両方を受け付け、未知version・tag混在・binding鍵不一致・trailing bytesを拒否する。[codec](../crates/protocol/src/membership.rs)・[回帰テスト](../crates/protocol/tests/membership_v2.rs)
- binding一意性はtransport種別ではなくbindingの鍵そのもので比較する。退役したcloudノードと別Node ID・別鍵の後継は区別され、同じ鍵の再利用は拒否される。既存のevaluateは同じNode IDの鍵・binding不一致を引き続きIdentityMismatchとして拒否する。
- peer登録JSONはversion 4と`transport: signed_https`（canonical origin）を追加した。v4はmembership signerとトップレベル`signing_key`を必須とし、Ed25519として使えない鍵を登録時に拒否する。v3（native TLS/iroh）とv2（署名なし）の受け入れは維持し、versionとbinding種別の取り違えは拒否する。[登録処理](../crates/net-native/src/app.rs)・[管理Web](../apps/admin/src/api.rs)・[経路テスト](../crates/net-native/tests/signed_https.rs)
- 独立Python generatorのHMC v1/v2 fixture 15件（元bytes・署名入力・ID）とRustの突き合わせを追加した。[fixture生成](../design_checks/membership_fixture.py)

### R35 — 署名HTTPSのchallenge交換とfreshness検証

- MembershipSignerに、署名HTTPSの5種類のframeだけを署名する型付き操作を追加した。任意のmessageへ署名するoracleは公開しない。検証は呼出し元が既にpinした鍵で行い、frame内の鍵を信頼しない。signing bytesは用途別domain分離済みのcanonical frameで、応答は交換hash・challenge hash・client/server nonce・generationへ結合する。[署名・検証](../crates/node/src/https_peer.rs)
- `POST /peer/v2/challenge`はchallenge requestの署名検証後にだけ発行し、未知の相手には行を作らない。同じ`(generation, requester, client_nonce)`の同一要求には同一challenge bytesを返し、TTLを延長せず行も増やさない。異なるcommitmentは拒否する。challengeは署名してからhashを保存する。
- `POST /peer/v2/exchange`は、保存済みの全binding・inner digest/length/class・期限・generationを照合し、`unused→used`をCASで消費してからnode handlerへ渡す。同時実行では最大1回だけdispatchする。handlerが失敗・退役・競合でも消費は戻さない。応答は消費した交換へ結合した署名付きHPSまたは限定codeの署名付きエラーで、HTTP statusと一致する。
- 状態は1 MiB以下の行数上限（要求元8件・全体256件）、req/全体のdurable token bucket、challenge TTL 60秒、期限切れ・消費済み・旧generation行の有界回収を持つ。消費済み行は即時回収し、8件上限がコマンド数上限にならないようにした。TTLはreplay hygieneで更新因果には使わない。
- 認証clockは、永続high-waterより戻ったwall clockをfail closedにし、runtimeのmonotonic経過で期限判定だけを前に進める。durable high-waterは観測wall timeの最大値のみ。cold startはgeneration・行・high-waterを保持し、明示的なgeneration resetだけがlive challengeを失効させる。
- SQLite schema v12にtransport状態の表を追加した。再起動・複製要求・競合commitment・消費後再送・期限切れ・generation reset・clock逆行・行数上限・token bucket・退役後のevidence許可とdata拒否を実SQLiteで検証した。[テスト](../crates/store-sqlite/tests/https_peer_service.rs)

### R36 — 署名HTTPSのPeer入口、native transport、DO adapter境界

- native用のv2入口を追加した。POSTのみ、固定2 path、query・Origin・Host一致・本文上限を検査し、pin解決と署名検証の前にchallenge状態を一切作らない。data classはpinに加えて両者の現在のActive状態を予備確認する（node handlerは自分のtransactionで再確認）。1接続の処理は期限付きで、接続数も制限する。[入口](../crates/net-native/src/peer_v2.rs)
- native outbound transportは、challengeを検証してからexchangeを送る。TLSは通常のWebPKI（hostname・期限・server-auth）を検証し、追加の所有者提供rootは完全なchain検証のtrust anchorとしてのみ使う。redirectは追わず、200以外・不正な署名・今回のattemptと結び付かない応答を拒否する。適用先originはpeerのbindingと完全一致させる。[transport](../crates/net-native/src/lib.rs)
- 実TLSのloopbackで、テスト用CAの完全なchain検証、challenge→exchange→HPS応答、未知の要求者拒否、別server鍵拒否を確認した。TLS検証を無効化した経路は存在しない。[実通信テスト](../crates/net-native/tests/peer_v2_https.rs)
- `harmion-worker` crateを追加し、DOのhost境界（同期transaction・rollback・`sync`）と、その上のdurable challenge store、HTTP型に依存しないPeer入口を実装した。worker crateは`wasm32-unknown-unknown`でcheckが通る。peer-onlyのDO node保存adapterはR38で追加した。Workers配線・bootstrap・secret・TLS終端・実Cloudflare配置は行っていない。[adapter](../crates/worker/src/lib.rs)・[テスト](../crates/worker/tests/entry.rs)
- 未検証: 実DO isolateのeviction中のchallenge CAS、実hostのtransactionSync/syncとの接続、実CloudflareのTLS・routing・quota・料金、実ネットワークのredirect/SSRF/プロキシ、Android/Windows、実deploy。ローカルloopbackとmemory hostの成功をCloudflare可用性の保証とは扱わない。

### R38 — DOのpeer-only node保存adapter

- `harmion-worker`に、Durable Objectの同期SQL境界上のpeer-only `Store`を追加した。record logとContent索引、peer cursor、membership log・cursor・root・回復root、peers、署名identity pinを実装し、Peer同期に必要な経路だけを提供する。App API、Query/Subscribe、OAuth、token、request receipt、sync観測は`Storage`エラーでfail closedし、存在しないデータを装わない。
- 64-bitの位置は8-byte big-endian BLOBで保存・比較し、JS Numberを経由しない。`seq`はtransaction内で最大値+1を採番し、hostのImmediate相当のtransactionで直列化する。record bytesは元のまま保持し、読み出し時にseq・update ID・digestを再検査する。
- nodeのtransaction errorはhostのrollback要求として渡し、host transactionが失敗を返した後に元の`Error`型で呼出し元へ戻す。`CursorMismatch`・`PeerRetired`・`IdentityMismatch`等の意味を`Storage`へ潰さない。
- 実SQLiteをhostの代役にしたテストで、challenge→exchangeのmembership/status、Accept→Ack→Page、再open後のlog・cursor・pin保持、別Node IDでのopen拒否、不整合pageのrollback（log/cursor不変）、App/Query面のfail closedを確認した。[adapter](../crates/worker/src/store.rs)・[テスト](../crates/worker/tests/worker_store.rs)
- 自己レビューでの限界: workerd/DOのJS SQL bridge（`sql.exec`相当）とWorkerのfetch・routing・TLS終端・bootstrap・secretは未実装。起動時の全履歴検証はDOでは行わず、読み出しごとのrecord検査とSQL制約に依存する（nativeのstartup検証と同等とは扱わない）。search projectionはno-opで、App APIを公開しないpeer-only前提。実DOのeviction・multi-region・課金は未検証。

### R37 — 署名HTTPS実装の自己レビュー

実装後に次の問題を発見し、修正した。

- native server/clientの乱数取得失敗がゼロnonceへfallbackしていた。challenge/交換のnonceが予測可能になるため、失敗をEntropyエラーとして扱い、challenge発行・exchangeを開始しないよう変更した。service層の乱数sourceは`Result`を返し、storage transaction内でも失敗を伝播する。
- `identity::pin`は直接呼出しではSignedHttps binding鍵と署名鍵の一致を強制していなかった。HMC v2のencode/decodeに加えてpin層でも拒否する。
- 旧DBでtrust root／回復rootの自己署名証明書がmembership logに重複していない場合、pin移行がCorruptLogで失敗し得た。rootのdurable状態から同一memberを特定できる場合はowner provenanceでseedするよう修正した（runtimeのpin挿入も同じ）。
- challenge routeの本文上限がexchange用の4 MiB+8 KiBだった。metadata frameの8 KiB上限をroute別に適用した。
- 登録v4の署名鍵をEd25519公開鍵として検証していなかった。使えない点を保存すると将来も認証できないため、登録時に拒否する。
- WebPKIのhostname検証を実通信テストへ追加した。別hostnameのテストCA証明書を提示するserverへの接続は失敗し、検証無効化や`harmion.local`上書きの経路が無いことを確認した。
- 自己レビューで残した制約: 同一challenge要求の完全な重複は行を増やさないがrate bucketも消費しない（接続数・期限・署名検証の上限内）。同時に同一exchangeを投げる並行試験は実threadでは行っていない（CAS文とImmediate transactionで最大1回になる設計）。実302応答の追従拒否は経路として存在しないが実serverでは未試験。worker hostの`sync`は呼出し側の責務で、memory host試験では物理flushを模擬していない。

### R39 — デプロイ時provisioningと読み取り専用Worker管理API

- host専用の`provisioning::provision`を追加した。所有者が渡す自分のMember、自己署名のtrust root、自分のadmission証拠、初期contactを検証してから、membership identity・root・証拠・pinを1つのtransactionで設定する。鍵とNode IDの一致、rootが自己発行anchorであること、全証明書の署名、signed-HTTPS bindingの鍵一致とcanonical origin、contactの重複を検査する。[実装](../crates/node/src/provisioning.rs)
- 同一manifestの再適用は冪等で成功し、異なるroot・identity・鍵は`IdentityMismatch`、無効な証拠は署名・bindingエラーとして失敗する。まだadmissionが無い場合は、明示pin済みcontactが1つ以上あるときに限りevidence交換のbootstrapとしてUnconfirmedを許す。鍵を暗黙に再生成しない。[回帰テスト](../crates/store-sqlite/tests/provisioning.rs)
- 同じprovisioningをpeer-onlyの`WorkerStore`上でも実行し、membership状態・root・pinが作成され、provision後に固定pinの相手とchallenge/exchangeできることを確認した。[Workerテスト](../crates/worker/tests/worker_store.rs)
- 読み取り専用のWorker管理APIを追加した。`capabilities`（mode=worker、read_only、画面・操作ごとの可否）、`provision-status`、`members`、`peers`、`overview`のみを許可し、未知operation・書き込みoperationは404で拒否する。Appデータ・競合・OAuth・Export・sync・Peer承認は「利用不可」として明示し、架空の値を返さない。[実装](../crates/worker/src/admin.rs)
- 管理画面の認証は、Cloudflare Accessを外層、デプロイ発行Bearerをアプリ層の二重防御とする方針を確定した。`bearer_matches`は定数時間比較で、期待tokenが未設定なら常に拒否する（fail closed）。読み取り専用の初版では、参加承認・退役・syncは無効化する。
- 現時点の未実装: 既存Svelte UIのcapability連動とBearer入力、workerd/DOのJS host bridge、Workerのfetch/static asset/TLS/routing配線、実Cloudflare配置。native adminの既存画面は変更していない。

### R40 — 管理画面UIのcapability対応と読み取り専用表示

- Svelte管理画面を、native管理サーバーとWorker管理APIの両方で使えるように拡張した。起動時に`/capabilities`を取得し、`sections`で画面タブを、`operations`で操作を出し分ける。native管理サーバーは`/capabilities`を提供しないため404となり、従来どおり全画面・書き込み有効のnativeモードとして動作する。[APIクライアント](../apps/admin/ui/src/lib/api.svelte.ts)
- Workerモードは、`/capabilities`が未認証で401を返すこと（native管理サーバーは404）で検出し、起動リンクではなく管理者トークン入力フォームを表示する。`window.__HARMION_ADMIN_MODE__ = "worker"`があればそれを優先する。トークンは`sessionStorage`に保持し、`Authorization: Bearer`で送る。Cloudflare Accessは外層で、アプリ層のBearerは必須のまま。[App](../apps/admin/ui/src/App.svelte)
- 読み取り専用モードでは、Contents・Conflicts・Apps・Exportのタブとページを描画せず、Peer画面も承認・参加・退役・手動同期・QR読み取りを出さない。代わりにrouting projection（transport・origin/endpoint・pin provenance・retired）と参加状態を読み取り専用で表示する。[Peer画面](../apps/admin/ui/src/pages/Peers.svelte)
- OverviewはWorkerの応答形（metrics/permissions/peer_transferがnull、peers配列、provisioning設定状態）を受け入れ、取得できない値を0や「有効」と偽らず「—」「設定済み/未設定」で示す。[Overview](../apps/admin/ui/src/pages/Overview.svelte)
- 検証: `npm run check`（svelte-check 0 errors）、`npm run format:check`、`npm run build`が成功し、Rustのworkspace buildで埋め込みアセットとして再生成される。native管理画面の起動リンク・セッション・書き込み系の挙動は変更していない。
- 実Cloudflare配置・Accessポリシー・secret配布・TLS終端は行っていない。workerd上の配線はR41で実装した。

### R41 — workerd/DOのJS SQL bridgeとWorker配線

- wasm32専用の`do_bridge`を追加した。JSの`storage.transactionSync`と`storage.sql.exec`を`SqlHost`/`KvHost`として接続し、同一の`transactionSync`境界でchallenge CAS・membership log・record pageを扱う。非`'static`のclosureはwasm-bindgen 0.2.130の`ScopedClosure`でJSへ渡し、Rustの`Err`をrollback用のthrowへ変換した後、元の`Error`型を保持して返す。BLOBは`Uint8Array`、64-bit位置は既存どおり8-byte big-endian BLOBで扱う。[bridge](../crates/worker/src/do_bridge.rs)
- challenge storeはDOの非同期KVではなく、同じSQL transaction内の`kv`テーブルに置いた。CASとrate bucketが同期transactionで原子になる。[host](../design_checks/worker_do/host-api.js)
- Worker配線は`/peer/v2/challenge`・`/peer/v2/exchange`、管理API（`/capabilities`等と`/admin/*`）、host専用`/provision`（bootstrap token必須）、静的UIアセットを提供する。各exportの後に`storage.sync()`をawaitしてから応答する。Bearerはアプリ層で検証し、未認証の`/capabilities`は401を返す。[worker](../design_checks/worker_do/worker.js)・[config](../design_checks/worker_do/config.capnp)
- 管理画面はWorkers Assetsとして配信され、`run_worker_first`でAPIのみWorkerへ送る。UIは401/404でWorker/nativeを判別し、Workerではトークン入力・読み取り専用表示になる（R40）。[probe](../design_checks/worker_do/check.py)
- 検証: 実workerd上で、bootstrap token不一致の拒否、未認証adminの401、UIアセット配信、provisioning、署名付きchallenge→exchange（MembershipStatus）、Accept→Ack→Page、workerd再起動後のlog high=1と管理APIを確認した。クライアントはnativeのRust exampleで、実際のPeer v2 frameを署名・検証する。[client](../crates/worker/examples/probe_client.rs)
- 自己レビューでの限界: ローカルworkerdのみで、Cloudflare Access・実secret配布・TLS終端・実アカウント配置は未検証。bootstrap tokenは固定のテスト値で、本番ではprovision後の無効化・Access・secret rotationが必要。毎リクエストでschema DDLとidentity解析を行うため、実運用ではisolate単位のキャッシュが必要。`globalThis`のhost差し替えは単一スレッド・非再入前提。transactionSyncのcommitを耐久点として扱い、commit途中のクラッシュ・電源断は未試験。実DOのeviction・multi-region・課金・起動時全履歴検証は未検証。管理APIのrate limitはAccess側に依存する。

### R42 — 登録コード、Worker発のoutbound同期、対称性

- Workerの登録コードを追加した。`/registration-code`（Bearer必須）はprovisioning済みseedから公開鍵を導出し、root・canonical originを含む`harmion-peer:v1:z:`コードを返す。Unconfirmedでも承認に必要な情報が揃う。[admin](../crates/worker/src/admin.rs)
- probeでコードをgzip+base64デコードし、native管理画面と同じJSON形・Node ID・署名鍵・signed-HTTPS originであることを確認した。[client](../crates/worker/examples/probe_client.rs)
- Worker発のoutbound同期を追加した。`outbound::sync`はnativeと同じ順序（membership evidence → Status/Cursor/Page/Accept）で、membership logとrecord cursorを更新する。store transactionはawaitをまたがない。[実装](../crates/worker/src/outbound.rs)
- transportは`fetch`ベースの`signed HTTPS`クライアント。httpsのみ（probeの`.internal` service bindingだけ例外）、redirect拒否、15秒timeout、challenge proofと応答bindingの検証、署名鍵不一致・自己peerの拒否を行う。wasmでは`wasm-bindgen-futures`のasync export `outbound_sync`、nativeではテスト用のscripted transportで同じcoreを検証する。[host](../design_checks/worker_do/host-api.js)
- `/internal/sync`（Bearer必須）からpeerを指定して1ラウンド実行できる。workerd probeでは未認証の401を確認した。scripted transportのRustテストでは、remote pageの取り込み（high/cursor/record一致）とmembershipのpushを確認した。[テスト](../crates/worker/tests/outbound_sync.rs)
- 対称性: node daemonは既に全peerへ同期を開始し、葉同士も直接同期できる。Workerは到達可能なpeerにだけ開始できる（NAT配下の葉には到達できない）。Workersは必須のハブではない。[同期ループ](../apps/daemon/src/main.rs)
- 限界: 同期観測のSQL永続化、実Cloudflareのegress・課金・rate limitは未検証。

### R43 — DO alarmによる定期outboundと2サービス実E2E

- `outbound_peers`と`outbound_sync_list`を追加した。ActiveかつretiredでないSignedHttps peerを列挙し、指定peerだけを順に同期する。Durable Objectのalarmが一定間隔で起動し、peerごとの最終試行・連続失敗を`storage`に保存して、成功時は次回間隔まで、失敗時は指数的バックオフ（最大30分）までスキップする。[実装](../crates/worker/src/outbound.rs)・[スケジューラ](../design_checks/worker_do/worker.js)
- workerdをhubとleafの2サービス構成にした。leafはhubのservice binding経由でしか到達できず、外向きoutbound専用の葉を模擬する。leafにrecordを投入し、hubのalarmがfetchでoutbound pullし、hubが別peerへ配信できること、直後の再alarmはバックオフで空になること、再起動後もhigh=1を保持することを確認した。[probe](../design_checks/worker_do/check.py)
- `wasm-bindgen-futures 0.4.80`は通常のcrates.io依存に戻した（サンドボックスの読み取り専用homeのため一時的に広い権限で`cargo fetch`を実行）。
- 限界: バックオフと最終試行はDO KVに保存しておりSQLの同期観測テーブルではない。alarmの最短間隔・同時実行・実Cloudflareの課金は未検証。native daemon↔WorkersのE2Eは未実施。

### R44 — 本番Worker資産、bootstrap無効化、production smoke

- デプロイ可能なWorkerを[deploy/cloudflare](../deploy/cloudflare/README.md)に追加した。`wrangler.toml`（Durable ObjectのSQLite migration、Assets、`run_worker_first`、vars）、ビルドスクリプト（wasm＋bindings＋管理UIを`src/pkg`・`public`へ）、本番用`worker.js`・`host-api.js`、運用手順を含む。生成されたbindingsの`host-api`を相対importへ書き換え、wranglerとworkerdの両方で解決できるようにした。
- provisioning完了後は`/provision`が`409 already_provisioned`でfail closedするようにした。同一manifestの意図的な再適用だけ`ALLOW_REPROVISION=1`で許可する。probeで2回目のbootstrapが拒否されることを確認した。
- 本番Workerのworkerd smoke testを追加した。モジュール読み込み、UI配信、未認証adminの401、誤bootstrapの401を確認した。[smoke](../design_checks/worker_do/prod_check.py)
- 運用手順（Accessの2アプリ構成、secret配布、custom domain、検証、rotation、コスト）をREADMEに記載した。Cloudflare AccessはUI/adminをSSOで保護し、`/peer/*`はBypass、`/provision`・`/internal/*`はBearer/bootstrapで保護する想定。
- 限界: `wrangler deploy`、実Accessポリシー、実secret配布、TLS終端、実アカウントでの課金・quotaは未実施。manifest生成CLI、secret自動rotation、DOのtime-travelバックアップは未実装。production smokeはビルド成果物をworkerdで読み込むだけで、実Cloudflareの挙動を保証しない。

### R45 — native daemonのsyncループをsigned-HTTPS TLSで検証

- `NativeTransport`と`sync::sync_once`を使い、2つの独立したSqliteStore（server/client）をprovisioningして、使い捨てCA＋`localhost`証明書のv2サーバーへ実TLSで接続するテストを追加した。clientのrecordがserverへpushされ（uploaded=1、server high=1）、次のラウンドで同じrecordをpullしてcursorが進むことを確認した。membership evidenceの交換も同じtransportで通る。[テスト](../crates/net-native/tests/sync_signed_https.rs)
- これでdaemon側の同期ループ、membership_sync、replication、SignedHttpsClient、WebPKI（追加trust anchor）が実TLSで組み合わさって動くことを確認した。Worker DO側のpeerはworkerd probeで別途検証済み。
- 限界: このテストの相手はnative v2サーバーであり、workerd DOをTLS終端の背後に置いた単一プロセスのnative↔Workers E2Eは未実施。daemon本体を設定ファイルから起動してのE2E（App/管理APIの初期化を含む）も未実施。

### R46 — native syncからworkerd DOへのTLS越しE2E

- 使い捨てCA＋`localhost`証明書でTLSを終端し、HTTP/1.1をローカルworkerdへ転送するprobe用フォワーダを追加した。native側は`NativeTransport`＋`sync::sync_once`のまま、workerdのPeer v2エンドポイントへ実TLSで接続する。[example](../crates/net-native/examples/tls_forward_sync.rs)
- workerd側は本番用Worker（`deploy/cloudflare/src`）を動的origin `https://localhost:<port>`で起動し、hubをprovisioningした。nativeがrecordをpushしてDOのhigh=1になり、次のラウンドで同じrecordをpullしてcursorが進むことを確認した。[driver](../design_checks/worker_do/native_proxy_check.py)
- これでnativeの同期ループ（membership_sync/replication/SignedHttpsClient/WebPKI）とWorkers DOのPeer入口・SQL保存が、単一プロセスで実際に組み合わさって動くことを確認した。
- 限界: フォワーダはprobe用でデプロイ対象ではない。実CloudflareのTLS・Access・secret配下ではない。daemonバイナリを設定から起動するE2E（App/管理API・管理画面の初期化を含む）は未実施。AlarmによるWorkers発のpullは別probe（R43）で検証済み。

### R47 — ハブ経由のmembership中継と複数peer検証

- workerd probeにmembership中継を追加した。rootが新しいadmissionをhubへ`MembershipAccept`で送り、hubが自身のlogへ追記し、別peerが`MembershipPage`でそのcertificateを取得できることを確認した。[probe](../design_checks/worker_do/check.py)
- 同じprobeで、root・hub・leafの3ノードのper-peer cursor、leaf→hub→rootのrecord中継、hub発のoutbound pull、alarmバックオフ、再起動保持をまとめて検証している。
- 限界: probeのpeerはnativeのRustクライアントで、daemonバイナリのApp/管理API初期化は通していない。実CloudflareのAccess/TLS配下ではない。hubのlogに追記されたmembershipの重複・長期的な肥大化は未評価。

### R48 — daemonのsigned-HTTPS追加trust anchor

- `harmiond run --signed-https-root <PATH>`（繰り返し可能）を追加し、DER証明書を追加のWebPKI trust anchorとして`NativeTransport`へ渡すようにした。空・過大・読めないファイルは起動前に拒否する。CLIのパーステストを追加した。
- これはprivate CA配下のreplicaにnativeノードから接続するための運用機能で、通常のCloudflare公開証明書では不要。
- 限界: daemonバイナリを実際に起動してpeer承認・同期まで通すE2Eは未実施（同期コアはR45/R46で検証済み）。

### R49 — Worker向けprovisioning manifestの生成と実daemon E2E

- `harmion_node::provisioning::export_manifest` と App API `provision-manifest`、CLI `harmion provision-manifest <JSON|code> [--out PATH]` を追加した。承認済みでActiveなSignedHttps Peerについて、root anchor、root-firstのAdmit証明書列、Activeなcontactsを返す。未承認・identity不一致・権限不足では発行しない。
- 実バイナリE2E [worker_daemon.py](../design_checks/worker_daemon.py) を追加した。実`harmiond`のfounder Aを起動し、Workerの登録コードをAで承認、manifestを生成してworkerdの`/provision`へ投入、Aからrecordをpush、別の実`harmiond` BがAから参加・pull、AがBのadmissionをWorkerへ伝播した後、BがWorkerだけから新規recordをpullする。A/B再起動後の同期と、2回目の`/provision`拒否も確認する。
- 限界: ローカルworkerdとloopback TLSで、実CloudflareのAccess/TLS/課金は未検証。E2Eはworkerd自体の再起動を含まない（DOの保存耐久はR41のprobeで別途確認）。Workerの`/provision`はhost専用のまま。連続した手動syncはprovisionalなchallenge/rate上限（burst 16、2/s）に達し得る。その場合App APIは429 `peer_capacity`を返し、E2Eは6秒待って再試行する。定期syncはバックオフで再試行する。

### R50 — edit tokenとrequest receiptの30日保持

- schema v13で`edit_tokens.created_at`と`request_results.created_at`を追加した。新規行はホスト時刻を保存し、30日より古い行をGCで回収する。receiptを先に削除し、basis、未参照のtokenの順で消す。方針導入前の`created_at=0`行は削除しない。
- 回収済み・不明なtokenでの保存は`TokenExpired`（HTTP 410 `token_expired`）にした。liveなreceiptが参照するtokenは保持し、同じrequest_idの再送は元の結果を返す。receipt回収後の再送は新しい更新になり得る。
- daemonは起動時と6時間ごとにGCする。テストは回収、live receiptの保持、方針前行の保持、回収後の再取得要求を確認する。
- 限界: 30日は所有者が選んだ下限方針で、tokenが実際に消えるのは次のGC。GCはログのみで管理APIの指標には出さない。旧行は回収しない。長期的なDBサイズ上限を保証するものではない。

### R51 — nativeのv2入口・大容量・鍵変更の扱い

- nativeはiroh/HTTPSのまま公開SignedHttpsピアにしない。WorkerのoutboundはSignedHttps以外をスキップするため現行の端末起点同期は成立する。`peer_v2::serve`は実装済みだがdaemonには配線せず、nativeへv4 SignedHttps登録を使ってadmitすると同期だけ失敗する点を運用上の注意として残す。単一`Member.binding`のためSignedHttpsとirohの併用はできない。公開Linuxピアが必要になった時点で実装する。
- 単一Contentは保存要求1 MiB、応答8 MiBを上限とし、stream/page取得はv1で提供しない。上限超過は413で失敗し、部分的な成功にしない。
- v1は署名鍵ローテーションを提供しない。既知identity pinは同一ノードの鍵変更を拒否するため、端末交換は新しいNode IDでの復元（R23）を使う。遠隔管理grantも提供しない（Peer資格を管理grantへ昇格しない）。これらは欠陥ではなくv1の対象外として明示する。

## 基準時点の複数ノード同期の検証条件

1. 3〜5ノードで、A↔B↔Cの中継、循環、分断中の別々の編集、後着の競合解決、再送が収束し、静止後のログ件数が増えない。
2. 1台の参加承認が他の信頼済みノードへ伝わる。別クラスタ・偽署名・Node IDへの鍵差替えを拒否する。
3. 退役が伝播し、再送で復活しない。退役元が承認した別ノードの参加資格を連鎖失効しない。
4. 管理情報の途中失敗・再起動・重複と受信順序を確認する。管理情報を正本ではなくキャッシュとして扱って消さない。
5. 1台が到達不能でも他のオンライン端末が同期を進める。実daemon・CLIで確認する。
6. 全体の既存テスト、fmt、Clippy、fixture整合性を通す。追加機能だけの狭いテストで全仕様準拠を主張しない。

## 基準時点の実行記録

- `cargo test --workspace --offline`: 成功。JSON属性の変形と、停止したPeerによる他Peerの待機に回帰テストを追加。
- `cargo clippy --workspace --all-targets --offline -- -D warnings`、`cargo fmt --all -- --check`: 成功。
- `design_checks/multi_process_sync.py`と`--iroh`: 最終版でも両方成功。5つのdaemonと実DBを使用。隣接ノードのapprove/joinから参加証明が行き渡るchain/ring、分断中の並行編集、解決後の後着編集、クラッシュ再起動、有限の再送、1台停止中の周期同期と復帰を確認。
- `membership_process_sync.py`と`--iroh`: 両方成功。4ノードで一度の参加承認の転送・退役伝播・確認済み子の維持・自己退役・相互退役・第三ノードからの確定／却下・クラッシュ後の状態保持を検証。
- 同スクリプトで、2ノードの相互退役から新しい回復ノードを所有者が明示確認して復旧する経路も成功。ローカルな回復trustを転送しただけでは相手が信用しないことを確認。
- 同スクリプトの別シナリオで、C→Bの退役証明を知らずにAがCを退役させてもBを自動復活させず、保留・停止から明示的な確定で退役できることを確認。
- Rustでは全78テストが通過。署名改変・別cluster・鍵置換・自己署名循環・受信順序・退役cutの固定・再承認・保留対象の自己解除拒否・証明ページの正常prefixのrollback・独立cursor・重複・再open・署名破損・ローカル鍵をクラスタ権限にしないことを含む。
