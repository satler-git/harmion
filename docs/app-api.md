# App APIの契約

更新日: 2026-11-18

会話で合意した取得・編集・購読・OAuthの契約。メソッド名・通知名は論理名であり、具体wire形式・型・期限・サイズは実装時に定める。実装済み機能の説明ではない。

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

保存成功はローカル永続化の成功であり、その値が通常表示に選ばれることを保証しない。保存結果は更新IDと保存完了を返し、Readがあれば保存直後の通常値・新しい編集トークン・競合情報も返す。Writeのみには本文を含まない受領結果を返す。SDKはアプリの編集バッファを勝手に上書きしない。

get_conflictsは権限内の候補と解決トークンを返す。resolveは解決する項目と候補を明示して新しい更新を作る。候補の一つをそのまま選ぶ場合も新しい解決更新とする。複数項目を一度に解決できるが、取得した全競合を自動的に対象にしない。未確認の並行更新は残す。

## 3. QueryとSubscribe

queryは一回の固定スナップショットをページ取得し、購読を作らない。期限切れなら最初から取り直す。Query ASTと基本演算子はspecificationの6.1を参照。

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

## 4. その他のAPI

capabilitiesはAPI版・対応操作・上限、permissionsは自分のgrantを返す。sync_statusは到達状況や複製確認を返し、ローカル保存成功と区別する。

Interpreterのcallは渡された入力を計算し、Query/Subscribeを暗黙に実行しない。デフォルトQueryとユーザー条件は通常ANDで絞り、置き換えは明示指定とする。提供元と呼び出し元の契約・型定義・エラー詳細は実装時に定める。

## 5. OAuthとローカル構成

各ノードが自分へのアプリ接続を認可する。原則public clientとし、埋込み共通client secretを使わない。Authorization Code＋PKCE S256、短命な不透明access token、発行するrefresh tokenのローテーションを基本とする。トークン・grantは発行ノード限定で、同期しない。

1. アプリを登録してclient_idを得る。登録だけではデータアクセスを許可しない。
2. 権限・redirect URI・PKCE challengeを指定して認可を開始する。
3. 所有者がHarmion側でアプリと要求権限を確認して承認する。
4. 短命・一回限りのコードを返し、アプリがverifierとともにトークンへ交換する。
5. App APIでaccess tokenと現在のgrant・ContentのFilterを検査する。

名前やclient_idだけでアプリの真正性を保証せず、同じ名前のアプリへ既存grantを自動付与しない。refreshによる権限拡大は認めず、追加権限は再承認する。grant縮小・失効は既存tokenとSubscribe/Provider接続へ反映する。

最初の管理grantはOSユーザーだけが使えるbootstrap手段から付属管理ツールへ渡す。その後は同じApp APIを使う。認可画面を開けるだけで承認できるようにはせず、所有者の認証状態に結び付ける。管理権限を認可対象アプリへ渡さない。

初期版は端末内のloopback HTTP/WSを選び、OAuthの認可・token endpointのTLS要件から外れるローカル専用構成として明記する。RFCのloopback HTTP redirectの例外が認可サーバー全体に適用される、とは解釈しない。外部通信はHTTPSまたはirohで暗号化する。完全なOAuth標準準拠を主張しない。

- loopbackのみで待受け、LAN公開を単純な設定変更では許可しない。
- SDKはOS保護された接続情報を使い、発見したポートだけを信用して保存済みtokenを送らない。
- Host/Origin/CSRF検査を行い、任意のWebサイトから管理操作させない。
- 永続tokenをURLやログへ残さない。
- 同一OSユーザーの悪意あるプログラムからの隔離は保証しない。PKCEを接続の暗号化・ノード本人確認の代替にしない。

このローカル構成を対象ブラウザ・OAuthライブラリが扱えるか、別OSユーザーからのアクセスをどう防ぐかは早期試作で検証する。接続情報の保護だけでポート再利用・なりすましの全問題を解決できるとは扱わない。

OAuth実装はoxide-authを第一候補、Rust SDKはoauth2を候補とし、採用条件はarchitectureの12節を参照。実装時にはscopeとFilterの符号化、redirect登録、期限、refresh再利用検出、失効と永続化の原子性を確定する。

参照: [RFC 6749](https://www.rfc-editor.org/rfc/rfc6749.html)、[RFC 8252](https://www.rfc-editor.org/rfc/rfc8252.html)、[RFC 9700](https://www.rfc-editor.org/rfc/rfc9700.html)。
