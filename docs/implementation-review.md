# 実装全体レビュー

確認日: 2026-11-18。対象は現在のworktree。仕様の出典は[specification.md](specification.md)、API契約は[app-api.md](app-api.md)。会話・設計文書の照合を記録した[review.md](review.md)とは別に、ここでは実コードの状態を扱う。

この文書は作業台帳。指摘を直した場合も削除せず、状態・修正・検証根拠を更新する。実装がない機能を「仕様違反の実装」と混同せず、暫定方式が仕様を満たしていない場合はその差を明記する。テスト数だけを完成判定にしない。

## 確認範囲

domainの更新受理・projection・属性値、protocolのrecord/App/Peer codecとJSON view、nodeの認可・保存・membership・replication、SQLite transaction/schema、HTTPS/iroh adapter、SDK、daemon lifecycle/周期同期、CLI、既存テストと2プロセス検証を読んだ。外部依存の全ソース監査、全OSでの実行、実インターネットのNAT/公開リレー試験は含めない。

## 仕様との対応

| 仕様 | 現状 | 根拠・不足 |
|---|---|---|
| §3 Content、項目別原子保存、固定編集元 | 実装 | `domain::{Graph,ContentSnapshot}`、`node::application::save`。tokenは保存済みbasisを復元する |
| §4 競合・削除・keep/restore | Core実装、管理ツール部分実装 | `Graph::accept/project`。CLIは本文中心。比較UI、属性/kind選択、三者マージ案はない |
| §5 全履歴保持・管理向け取得 | 部分実装 | 不変recordは保持。inspectは現在の削除状態と候補を返すが、共通祖先の専用取得はない |
| §6 Query/Subscribe | 未実装 | node/APIに操作がない。getはID指定のみ。capabilitiesはfalse |
| §6 Read/Write/Manage権限 | 実装 | `node::access/application`。grant/credential/token/request_resultは同期対象外 |
| §6 OAuth・SDK購読 | 未実装 | 管理bootstrapと手動issue-appが先行。OAuth認可フローの代替完成品とは扱わない |
| §6 OS所有者保護 | Unix実装、Linux検証 | `local::ProtectedDir`。Windows/Androidの保護・lifecycleはない。同一OSユーザーを信頼する前提は仕様と一致 |
| §7 Interpreter | 未実装 | 登録、仲介、Call/Provide、Provider接続なし |
| §8 参加・退役の伝播 | 実装・検証済み | 共通の初期信頼証明、署名付きAdmit/Retire/Dismiss、管理ログ・保留・所有者回復。R01参照 |
| §9 端末間iroh・通信と意味の分離 | 実装、検証範囲限定 | `sync::PeerTransport`とHTTPS/iroh adapter。ローカルIP変更、リレーのみ、独立2プロセスを検証 |
| §9 同期契機 | 部分実装 | 起動・周期・手動・backoffあり。保存直後、OS復帰、更新通知は未接続 |
| §9 Cloudflare | 未実装 | worker crate/DO adapterなし。現在のHTTPSをCloudflare対応済みと呼ばない |
| §10 Export | 未実装 | 初期仕様に必要。DBファイルのコピーやHRQ要求ファイルはexportではない |

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

### R03 — 全ログの再読込・再構築を毎回行う

- 種別: 参照実装としての暫定方式。優先度: 中。状態: 残す。
- 根拠: `storage::read_graph/accept_records`、`replication::high/page`は`tx.entries()`を使う。SQLite entriesは全recordをdecodeする。受理時も既知IDのmapを全体から作る。管理情報も全ログから参加状態を再構築し、署名を再検証する。
- 影響: 履歴が増えるとCPU・メモリ・transaction保持時間が増える。5台上限でもデータ量の上限にはならない。
- 解消条件: seq範囲・update ID照会用port、永続projection/索引、性能測定。全履歴保持自体は初期仕様どおりであり、勝手な履歴削減で回避しない。

### R04 — JSON返却を全生成してからサイズを制限する

- 種別: リソース制限の不足。優先度: 中。状態: 未修正。
- 根拠: `app::response`は`serde_json::to_vec`の後に8MiB制限。`application::get/read_conflicts`では先に全snapshotとtokenを作る。
- 影響: 各recordが1MiB以内でも、長い履歴で増えた属性・競合候補の集合は8MiBを超え得る。返却は拒否できるが生成メモリを制限できず、取得でtokenだけが増えることもある。
- 解消条件: 読出し集合・候補のページング/予算、JSON生成時の上限。保存済みデータを切り捨てて成功応答にする回避は禁止。

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
- 制約: ブラウザ・OAuth・WS対応を実装したことにはならない。後からnative専用経路を一般アプリ契約と混同しない。

### R08 — 管理CLIは本文中心で、一般アプリSDKも薄い

- 種別: 未実装／暫定UX。状態: 残す。
- 根拠: CLIのedit/resolve/restoreはBodyのみ。restoreは既知属性/kindを消す。SDKのcallはJSON Valueを返し、型付き取得結果・OAuth・購読はない。
- 影響: Coreの属性/kind操作が存在しても、標準ツールで全操作できるわけではない。restoreの省略値を保持するようCoreの意味を変えて隠さず、現在の制約をhelp/手順に明記する。

### R09 — エラーの診断情報がAPI境界で失われる

- 種別: 暫定実装。状態: 残す。
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

## 今回の変更に対する検証条件

1. 3〜5ノードで、A↔B↔Cの中継、循環、分断中の別々の編集、後着の競合解決、再送が収束し、静止後のログ件数が増えない。
2. 1台の参加承認が他の信頼済みノードへ伝わる。別クラスタ・偽署名・Node IDへの鍵差替えを拒否する。
3. 退役が伝播し、再送で復活しない。退役元が承認した別ノードの参加資格を連鎖失効しない。
4. 管理情報の途中失敗・再起動・重複と受信順序を確認する。管理情報を正本ではなくキャッシュとして扱って消さない。
5. 1台が到達不能でも他のオンライン端末が同期を進める。実daemon・CLIで確認する。
6. 全体の既存テスト、fmt、Clippy、fixture整合性を通す。追加機能だけの狭いテストで全仕様準拠を主張しない。

## 今回の実行結果

- `cargo test --workspace --offline`: 成功。JSON属性の変形と、停止したPeerによる他Peerの待機に回帰テストを追加。
- `cargo clippy --workspace --all-targets --offline -- -D warnings`、`cargo fmt --all -- --check`: 成功。
- `design_checks/multi_process_sync.py`と`--iroh`: 最終版でも両方成功。5つのdaemonと実DBを使用。隣接ノードのapprove/joinから参加証明が行き渡るchain/ring、分断中の並行編集、解決後の後着編集、クラッシュ再起動、有限の再送、1台停止中の周期同期と復帰を確認。
- `membership_process_sync.py`と`--iroh`: 両方成功。4ノードで一度の参加承認の転送・退役伝播・確認済み子の維持・自己退役・相互退役・第三ノードからの確定／却下・クラッシュ後の状態保持を検証。
- 同スクリプトで、2ノードの相互退役から新しい回復ノードを所有者が明示確認して復旧する経路も成功。ローカルな回復trustを転送しただけでは相手が信用しないことを確認。
- 同スクリプトの別シナリオで、C→Bの退役証明を知らずにAがCを退役させてもBを自動復活させず、保留・停止から明示的な確定で退役できることを確認。
- Rustでは全78テストが通過。署名改変・別cluster・鍵置換・自己署名循環・受信順序・退役cutの固定・再承認・保留対象の自己解除拒否・証明ページの正常prefixのrollback・独立cursor・重複・再open・署名破損・ローカル鍵をクラスタ権限にしないことを含む。
