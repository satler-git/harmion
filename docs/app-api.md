# App APIの契約

更新日: 2026-11-20

現在のnative実装では、証明書を固定したloopback HTTPSの`POST /app/v1/{operation}`を使う。保存要求はHRQ-v1で、返却値は明示的なJSON view。App APIはHostを検査しOrigin付き要求を拒否する。ブラウザで行うOAuth認可には専用のHTTPS入口を使う。有効にしたノードのcapabilitiesは `oauth: true` と `oauth_origin` を返す。

会話で合意した取得・編集・購読・OAuthの契約。公開済みの Query・Subscribe の転送形式はそれぞれの節に記録する。未公開のメソッドの転送形式・期限・サイズは実装時に定める。

## 1. 操作の分類

| 分類 | 操作 |
|---|---|
| Content | get、create、edit、get_conflicts、resolve、delete、keep、restore |
| 読み出し・購読 | query、subscribe、resume、unsubscribe |
| Interpreter | list、describe、call、cancel、提供元の登録・接続 |
| 接続状態 | capabilities、permissions、sync_status |
| 管理 | アプリ認可・失効、ノード追加・退役、export |

管理操作も共通App APIで必要な権限を検査する。OAuthの認可・トークンendpointはOAuthのHTTP契約を使う。利用アプリの入口とノード間Peer APIは区別する。

## 2. 取得と編集

get/queryのContentは、ID、通常本文、属性、kinds、不透明な編集トークン、公開可能な競合情報を返す。競合候補の詳細は別取得とする。部分取得を導入する場合、取得していない項目を暗黙に編集元へ加えない。

editはContent ID、編集トークン、request_id、変更項目を受け取る。属性のset/unsetとkindの所属変更を明示し、省略項目は変更しない。配列は全体をsetする。

- 古い編集元でも有効なら受理し、必要なら競合にする。
- 解釈できないトークンはエラーにし、最新の編集元へ差し替えない。
- 編集トークンは使い捨てにしない。同じ編集元から別要求を保存すると別の枝になり得る。
- 同一request_idでの同じ要求は元の保存結果を返す。別要求への使い回しはエラー。
- トークンは権限を付与しない。保存時に現在のWriteを検査する。
- 編集トークンとrequest receiptは、作成から30日を下限に保持し、その後回収する。回収済みまたは不明なトークンでの保存は HTTP 410 と `{"error":"token_expired"}` を返すので、アプリはgetで取り直す。receiptが回収された後の同じrequest_idの再送は、元の結果を返せず新しい更新になり得る。方針導入前の既存行は削除しない。

保存成功はローカル永続化の成功であり、その値が通常表示に選ばれることを保証しない。HTTP 200 の保存結果は `update_id` と `saved: true` を返す。Readがあり、応答の上限内に収まるときは、`view` に保存直後の通常値・新しい編集トークン・競合情報を返す。閲覧できる `view` が上限を超える場合は `view: null` と `view_error: "response_too_large"` を返す。同じ要求を再送しても、元の更新IDと保存完了を取得できる。SDKは編集バッファを保持する。

nativeのget・conflicts・inspectは、認可・可視性を検査した後、返す内容全体のJSONサイズを編集トークン発行前に計測する。8 MiBを超える場合は HTTP 413 と `{"error":"response_too_large"}` を返し、新しい編集トークンやそのbasisを保存しない。本文・属性・競合候補を一部落として成功にはしない。最終JSONの符号化も書き込みごとに上限を検査する。これは応答生成と失敗した取得によるDB増加の制限であり、Contentの全履歴読込みやprojection全体のメモリ上限を保証するものではない。保存要求は上記のとおりreceiptを返し、大きなviewを理由に保存済み結果を失敗扱いしない。

保存要求の本文はrecord上限の1 MiBまでとする。単一Contentのストリーム／ページ取得はv1では提供せず、応答上限を超えるContentは413で失敗する。これは未実装の契約であり、部分的な取得を成功として扱わない。

get_conflictsは権限内の候補と解決トークンを返す。resolveは解決する項目と候補を明示して新しい更新を作る。候補の一つをそのまま選ぶ場合も新しい解決更新とする。複数項目を一度に解決できるが、取得した全競合を自動的に対象にしない。未確認の並行更新は残す。

### 競合候補の選択と本文の三者マージ

保存の `Select` は、解決または管理トークンが固定した候補の版 ID を指定する。置き換える版 ID も列挙し、選択した版をそこに含める。属性の unset と null、kind の所属と非所属はそれぞれ候補の値をそのまま使う。kind の選択と削除済み Content の復元には全権限と管理画面で取得した範囲が必要。保存要求の形式は[HRQ-v1](../contracts/app-save-v1.md)を参照。

CLI の選択 JSON は次の形を使う。版 ID は実際の64桁の16進値に置き換える。

```json
[
  {
    "field": {"type": "attribute", "namespace": "wiki", "key": "title"},
    "version": "SELECTED_VERSION",
    "replaces": ["SELECTED_VERSION", "OTHER_VERSION"]
  }
]
```

`POST /app/v1/merge-input` は所有者専用。32バイトずつの Content ID、`inspect` のトークン、左・右の本文候補の版 ID、取得対象を指定する1バイトが続く。取得対象は `0` が共通本文版、`1` が明示した共通本文版（追加の32バイトが続く）、`2` が左、`3` が右。いずれもトークンが固定した2つの本文候補を検査する。

共通本文版は、2候補の因果履歴の共通部分における本文候補。返却は `status: "body"`、`version`、`text`。複数ある場合は `status: "choose_base"` と `versions` を返し、共通本文版がなければ `status: "no_base"` を返す。Rust SDK は取得した3つの本文から行単位の三者マージ案を作り、重なる変更を競合マーカーで示す。案の生成は履歴を更新せず、利用者が確認した本文と置換対象を指定して既存の `Resolve` で保存する。

## 3. QueryとSubscribe

queryは一回の固定スナップショットをページ取得し、購読を作らない。期限切れなら最初から取り直す。Query ASTと基本演算子はspecificationの6.1を参照。

### ネイティブの Query

- `POST /app/v1/query` は HQU v1 の開始要求または継続要求を受け取る。Rust SDK の `query` と `query_next` が型付き要求を符号化する。[コーデック](../crates/protocol/src/query.rs)
- 開始時に Content ID の昇順で一致集合と編集元を固定する。各ページは取得時の資格情報で、固定時と現在の可視状態への Read を検査する。権限外や削除済みになった項目を飛ばすため、空のページでも `next` があれば続きがある。各項目の編集トークンは固定時の編集元に結び付く。
- 応答は `items` と `next` を持つ JSON。項目は get と同じ view。`next` はアプリに結び付いた不透明なカーソルを16進で表し、終端は `null`。カーソルの期限は開始から最大5分。別のアプリのカーソルや失効・未知のカーソルには HTTP 410 を返す。資格情報の失効は HTTP 401。
- ページごとの候補数は1〜32。応答のサイズ上限内でページを区切り、1件が上限を超える場合は HTTP 413。ノード全体で固定結果を同時に64件、版の参照を合計100万件まで保持し、上限超過では HTTP 429 を返す。期限切れの固定結果は新しい Query の開始時に削除する。

### 条件の評価

型付き条件は `and`、`or`、`not`、kind所属、最終編集時刻の大小比較、名前空間とキーで指定する属性条件からなる。空の `and` は真、空の `or` は偽。QueryとSubscribeは通常表示に選ばれた値を評価する。現在のRead Filterを独立に適用し、削除済みContentを結果から除く。削除と編集が競合中なら生存側の通常値を評価する。

最終編集時刻は、通常表示に選ばれた項目版の `edited_at` の最大値。採用された属性のunsetやkindの非所属も、項目の版として計算に含む。削除やkeepでは項目の版を更新しない。復元では復元後に選ばれた項目の版を使う。比較値はUnix epochからの符号付きUTCマイクロ秒。ローカルの日付や時刻は、指定したタイムゾーンに従ってクライアント側で時刻範囲に変換する。

時計のずれがある端末からの編集で、通常表示の版が以前より古い時刻の版に置き換わることがある。その場合、最終編集時刻も戻り、時刻条件への一致が変わる。購読の追跡には購読の順序情報と再開カーソルを使う。

| 属性条件 | 評価規則 |
|---|---|
| `exists` | キーがあれば真。`null` と空文字を含む |
| `eq` | 型変換を行わず、属性値の構造的な等値で比較する。数値は正確な10進値、配列は順序と重複を含む。オブジェクトのキー順は評価に影響しない |
| `lt` / `lte` / `gt` / `gte` | 数値同士は正確な10進値、文字列同士はUnicodeのコードポイント順で比較する。型が一致しない値、`null`、真偽値、配列、オブジェクトでは偽 |
| `contains` | 属性が配列で、指定値に構造的に等しい要素があれば真 |
| `contains_any` / `contains_all` | 属性が配列で、指定値のいずれか／すべてと等しい要素があれば真。指定値の重複で必要な要素数は増えない。指定値が空なら `contains_any` は偽、`contains_all` は配列に対して真 |

属性が存在しない場合、その属性の条件は偽。`not` は評価結果を反転する。Read Filterと削除状態は `not` の対象外。演算子名は論理名で、転送形式はページング契約と合わせて定める。

### 購読

subscribeには条件を渡す。初期版は条件に一致する集合を購読し、order_byや上位N件の維持は含めない。SDK・アプリ側で並べ替えと表示件数を扱う。

| 通知 | 意味 |
|---|---|
| snapshot_begin | 購読IDと初期取得開始 |
| snapshot_chunk | 固定スナップショットのContent群 |
| snapshot_end | 初期取得完了と再開カーソル |
| changes | upsert/removeのまとまりと再開カーソル |
| reset_required | 続きから追いつけないため初期取得を要求 |

初期スナップショットの基準点と以後の変更追跡を欠落なく接続する。初期取得中の更新はsnapshot_end後に反映する。初期キャッシュは完了まで仮状態と識別し、途中で切断した場合は初期版では取り直す。

upsertは通常取得と同じContentスナップショットで置き換える。removeは以前見えていたIDだけを渡し、削除・条件不一致・権限外の詳細や非公開内容は渡さない。短時間の途中更新は集約でき、通知は重複し得る。古い通知で状態を巻き戻さない順序情報を付ける。

SDKは通知を適用してから対応カーソルを保存する。購読キャッシュの編集トークンが更新されても、開いている編集画面の編集元は変えない。

再開カーソルはノード・アプリ・条件・購読世代に結び付く不透明値とし、ノード間同期ログ番号と分ける。resumeは最後に適用したカーソルを受け取り、必要な状態が残っていれば再開する。再起動・期限・保持上限等で継続できなければreset_requiredを返す。遅い購読者の通知を無制限に溜めない。

送信時にも認可する。kind変更による権限外はremove、grant変更は購読の再取得、資格情報失効は購読終了と再認証要求で扱う。固定スナップショットも開始時の権限を永久保持せず、送信キューの古い本文を権限外になった後に送らない。

#### ネイティブの Subscribe

- `POST /app/v1/subscribe` は、ページ候補数32の HQU v1 開始要求を受け取り、`snapshot_begin` を返す。開始時に固定結果と、変更追跡を開始する位置を同じトランザクションで取得する。
- `POST /app/v1/resume` の本文は購読 ID の32バイトと、直前に適用したカーソルの32バイトを順に並べる。JSON 応答の `type` に通知名が入り、購読 ID、カーソル、Content ID は16進。初回は `snapshot_chunk` の `items` を受け取り、`snapshot_end` 後に通常のキャッシュとして使える。
- 変更通知は `changes` の `upserts`、`removes`、`sequence`、`cursor` で表す。順序番号は購読内で増加する符号なし整数を10進文字列で表す。同じ購読の `resume` は直列に呼び出し、受け取った変更を適用してから次のカーソルを使う。応答を失った場合は、最後に適用したカーソルから最新状態を取り直せる。
- 変更がなければ最大20秒待つ。待機枠が埋まっている場合や期限までに変更がない場合は `idle` を返す。`POST /app/v1/unsubscribe` は購読 ID の32バイトを受け取り、呼び出し元の購読を終了する。
- 購読の状態はノードプロセス内に保持する。再起動、期限経過、grantの変更、保持範囲を外れたカーソルでは `reset_required`。資格情報の失効は HTTP 401。初回取得の期限と完了後の無通信期限は最大5分。同時購読16、購読ごとの可視 Content 10万、ノードの変更履歴4096件が上限。
- Rust SDK は `subscribe`、`subscription_next`、`unsubscribe` を提供する。初回取得中のキャッシュは仮状態として保持し、完了時に公開する。`checkpoint` は適用済みのカーソルとキャッシュを一緒に取得する。初回取得中に通信が失敗・中断した場合は、新しい購読で取り直す。[SDKの購読](../crates/client/src/subscription.rs)

## 4. その他のAPI

### ノードの状態指標

`POST /app/v1/metrics` は本文なしで呼び、全管理権限を必要とする。Content の総数 `contents`、表示中の数 `visible_contents`、受理済み更新数 `accepted_updates`、更新レコードのバイト合計 `record_bytes`、期限内かつ失効していないアプリ登録数 `active_apps` を返す。数は精度を保つため10進文字列。ノードの同一トランザクションで集計し、DB の余白・索引サイズやプロセスの CPU・メモリ使用量は集計範囲外。

### 管理 Web アプリの接続コード

Peer の接続コードは `harmion-peer:v1:z:BASE64URL`。ペイロードは公開 `PeerRegistration` の UTF-8 JSON を gzip 圧縮し、パディングなし Base64url で符号化したもの。展開後のデータは1 MiBまで。圧縮なしの `harmion-peer:v1:BASE64URL` も読み取れる。管理 Web アプリの `peer-preview` はクラスタ・Node ID・公開鍵を表示する単回の確認値を作る。変更を行う `peer-confirm` はその値を消費し、既存の App API の参加・承認処理を呼ぶ。読み取り・プレビューだけでは参加状態を更新しない。

登録JSONのversion 3はnativeのTLS証明書またはiroh Endpoint IDを`binding`に持つ。version 4は`binding: {"transport": "signed_https", "origin": "https://..."}`を持ち、トップレベルの`signing_key`を必須とする。originは標準URL parserでcanonicalなHTTPS DNS originとして検証し、署名鍵はEd25519の公開鍵として使えることを確認する。versionとbinding種別の取り違え、Ed25519として無効な鍵、非canonicalなoriginは登録前に拒否する。v4の承認はHMC v2の`SignedHttps` bindingと署名identityのpinを作る。v3の既存経路は維持する。[署名HTTPSのdraft](../contracts/https-peer-v2-draft.md)

アプリの登録コードは `harmion-app:v1:BASE64URL`。`name` と `redirect_uri` の JSON を同じ方式で符号化する。アプリが認可を開始した後は、認可画面の `harmion-auth:v1:REQUEST_ID` を所有者が管理画面に貼り付ける。`POST /app/v1/oauth-preview` は32バイトの要求IDを受け取り、ManageApps 権限を検査した上で要求元と権限を返す。所有者の確認後は既存の認可画面で許可・拒否し、登録済み callback へ戻る。

### Worker向けprovisioning manifest

`POST /app/v1/provision-manifest` は、ManageNodes を持つ所有者が、すでに承認済みでActiveな SignedHttps Peer の v4 登録JSONを本文に指定して呼ぶ。応答は Worker の `/provision` にそのまま渡せる JSON で、`own`（対象のmember）、`root`（クラスタanchorの証明書、16進）、`evidence`（anchorから対象までのAdmit証明書列、root-first、16進の配列）、`contacts`（現在Activeな他のmember）を含む。秘密は含まない。

対象が未承認・未Active・別identityの場合は発行しない。権限不足は HTTP 403、資格情報の失効は HTTP 401。CLI は `harmion provision-manifest <登録JSONファイル|harmion-peer:v1:z:コード> [--out PATH]` で、`--out` を省略すると標準出力へ返す。承認（`approve`）の後に実行し、得たJSONを `/provision` のbootstrap要求へ渡す。再適用の冪等性と競合拒否は `/provision` 側の契約に従う。

capabilitiesはAPI版・対応操作・上限、permissionsは自分のgrantを返す。

### 所有者向け同期状態・更新の受領確認

`POST /app/v1/sync-status` は、本文なしでノード全体の状態、または32バイトの更新 ID を本文に指定してその更新の受領確認を取得する。Content ID ではなく、保存結果の `update_id` を指定する。初版は全管理権限を持つ所有者に限定し、一般アプリへのPeer情報公開や保存結果単位の公開権限は導入しない。権限不足は HTTP 403、失効・期限切れ資格情報は HTTP 401、未知の更新 ID は HTTP 404、不正な本文長は HTTP 400。

- 応答は `node`、`local_epoch`、`local_high`、指定した `update`（未指定は `null`）、既知のPeerごとの `peers`。IDとepochは16進、件数・位置・UTCマイクロ秒の時刻は10進文字列。
- 各Peerは `node`、`retired`、`last_success_at`、`last_failure`、`local_epoch`、`acknowledged_through`、`remote_epoch`、`remote_high`、`received_through`、`unconfirmed_local`、`update_confirmed` を持つ。`retired` は保存されたPeerの退役フラグであり、参加状態の全情報や現在の到達性ではない。保留等の参加状態は `members` で確認する。
- 自発同期の固定highラウンドが完了したときに、認証済み相手のCursor／ACKが示した自分のログ受領位置と、相手ログの取得位置を保存する。`last_success_at` はその確認時刻。移行前、未同期、異なる自分のログepochの観測は受領証拠にしない。受信側として同期しただけでは、相手による自分のログの受領位置を推測しない。
- `update_confirmed: true` は、指定更新の自分のログ位置まで、そのPeerが永続受理したことを最後の成功ラウンドで確認できた、という意味。`false` は確認位置が足りないこと、`null` は更新未指定または有効な観測がないこと。`false`／`null` は未受領の証明ではない。中継経由での受領やACK喪失があり得る。
- `unconfirmed_local` は確認位置より後にある自分の受理済み更新の件数。ログには中継更新も入り、欠番もあり得るため、ログ位置の差ではなく件数を数える。実際の未配達数ではない。初回同期やpullでラウンド開始後に増えた更新は、成功したラウンドの直後でも未確認になり得る。
- 最新の自発同期が失敗した場合、`last_failure` に `attempted_at`（失敗を記録した時刻）と限定された `code` を残す。過去の成功観測は消さない。次の成功で `last_failure` を消す。時刻の大小で成否を決めないため、時計が戻っても最新の完了結果を区別できる。
- `code` は `transport_error`、`capacity`、`timeout`、`invalid_response`、`record_rejected`、`peer_retired`、`membership_conflict`、`unauthenticated`、`identity_mismatch`、`storage_error`、`sync_error`。資格情報、本文、接続先、外部由来のエラーメッセージは保存・公開しない。
- 取得はローカル状態を読み、ネットワーク接続や同期を起こさない。手動・自動の自発同期が観測を更新する。途中失敗したラウンドの一部ACKは初版では新しい成功観測にしない。観測のDB保存要求を出す前のキャンセルや、保存がコミットされない終了・保存失敗では以前の観測が残る。DB workerへ保存を依頼した後は、呼出し元がキャンセルされても保存が完了し得る。その場合も実際の保存完了まで同一Peerの自発同期を直列化し、古い結果が新しい試行を上書きしない。受領済みの更新や受信cursorを取り消す処理ではない。
- 観測はSQLite schema v10で保持し再起動後も取得できる。バックアップを新しい Node ID で復元したノードには引き継がない。過去の受領確認は現在オンラインであること、第三ノードへの伝播、将来のバックアップ耐久性、クラスタ全体の最新性を保証しない。何台で保存完了とするかというポリシーも追加しない。

Rust SDK は型付き `Client::sync_status(Option<UpdateId>)`、CLI は `sync-status [--update UPDATE_ID]` を提供する。ローカル保存成功のreceiptとは独立した照会とする。

ネイティブの管理者は `POST /app/v1/export` で固定時点の本文、属性・kind、競合候補、削除記録を取得できる。転送形式とCLI出力は [Export v1](../contracts/export-v1.md) を参照。

Interpreterのcallは渡された入力を計算し、Query/Subscribeを暗黙に実行しない。デフォルトQueryとユーザー条件は通常ANDで絞り、置き換えは明示指定とする。提供元と呼び出し元の契約・型定義・エラー詳細は実装時に定める。

## 5. OAuthとローカル構成

各ノードがnative public clientを Authorization Code＋PKCE S256 で認可する。登録、grant、コード、トークンは発行ノードに保存する。管理権限はOS所有者のbootstrap経路で設定し、OAuthはReadとWriteを発行する。

### 登録と認可

1. ManageAppsを持つ所有者がApp APIの `oauth-register` を呼ぶ。JSONに `name` と `redirect_uri` を指定し、応答の `client_id`、`authorization_endpoint`、`token_endpoint` をアプリに渡す。登録時の権限は空。
2. アプリがブラウザで `GET /oauth/authorize` を開く。必須パラメーターは `response_type=code`、`client_id`、`redirect_uri`、`state`、`scope`、`code_challenge`、`code_challenge_method=S256`。アプリは高エントロピーのverifierとstateを生成し、受け取ったstateを照合する。
3. 所有者が画面に表示された要求IDをCLIの `oauth-consent` に渡し、応答のリンクをブラウザで開く。リンクは要求と管理資格情報に結び付け、2分以内かつ一度の承認・拒否に限って使える。画面で名前、client ID、callback、grantを確認する。元の要求の期限は5分。
4. 承認時はcallbackに `code` と `state` を返す。拒否時は `error=access_denied` と `state` を返す。
5. アプリは `POST /oauth/token` にフォーム形式で `grant_type=authorization_code`、`client_id`、`code`、`redirect_uri`、`code_verifier` を送る。redirect URIは手順2と同じ文字列にする。応答の `access_token` をApp APIへの `Authorization: Bearer ACCESS_TOKEN` に使う。応答には `token_type=Bearer`、`expires_in`（秒）、`refresh_token`、`scope` も含まれる。

callbackには `http://127.0.0.1:PORT/PATH` や `http://[::1]:PORT/PATH` など、IPリテラルのloopback HTTPを使う。登録時と認可時でIP、パス、クエリを照合し、アプリはOSが選んだポートを認可時に指定できる。callbackのHTTP例外はRFC 8252に従う。認可、トークン交換、App APIはHTTPSで行う。

scopeは空白区切りで `read:all`、`write:all`、`read:kind:BASE64URL`、`write:kind:BASE64URL` を指定する。`BASE64URL` はkind名のUTF-8をパディングなしで符号化する。kind集合には同じ操作のkind scopeを複数指定する。同じ操作のallとkindは排他的。grantは登録単位で、異なるscopeへの再承認は既存の資格情報を失効させる。管理者はApp APIの `change-grants` で縮小・失効できる。拡大には改めて所有者の承認を要する。

### トークン

コードは60秒、access tokenは最長10分、refresh tokenの系列は初回発行から最長30日。refreshは `grant_type=refresh_token`、`client_id`、`refresh_token` を同じtoken endpointに送る。消費済みrefresh tokenを再送すると、その系列のaccess tokenとrefresh tokenが失効する。有効期限内の消費済みコードの再送も、同じクライアント・redirect・PKCEを照合できた場合は、そのコードが発行した系列を失効させる。消費、ローテーション、再利用時の失効は発行ノードのDBトランザクションで処理する。App APIとSubscribeは資格情報の期限と現在のgrantを検査する。

### ブラウザの信頼

ブラウザ用HTTPSは `127.0.0.1` に限定した専用リスナーで、既存App・Peer用の証明書と分ける。所有者がノードで生成した専用CAを直接取得し、使うブラウザでWebサイト識別用に明示的に信頼する。CAはloopbackのIP制約と下位CAの禁止を持ち、署名鍵はサーバー証明書を1枚発行した後に破棄する。ノードに残るのは有効期間1年のサーバー証明書とその秘密鍵、および公開CA証明書。導入・更新手順は[native実行手順](native-quickstart.md#oauthを有効にする)を参照。

認可サーバーはHostを検査し、承認操作には同一originと一時リンクを要求する。トークン交換はnativeアプリからのHTTPSリクエストで行う。アプリはトークンをOSで保護し、URLやログへ載せない。クライアント名とIDは登録を識別するための値であり、所有者は自分が開始した要求を確認する。同じOSユーザーとして動くプログラムは所有者と同じ信頼境界にある。動的クライアント登録、ブラウザからの直接App API、外部公開用の認可サーバーは今後の範囲とする。

[RFC 6749](https://www.rfc-editor.org/rfc/rfc6749.html)、[RFC 7636](https://www.rfc-editor.org/rfc/rfc7636.html)、[RFC 8252](https://www.rfc-editor.org/rfc/rfc8252.html)、[RFC 9700](https://www.rfc-editor.org/rfc/rfc9700.html)。
