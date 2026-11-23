# Interpreter v1 契約

更新日: 2026-11-23

この文書は、一つのHarmionノード内でInterpreterの登録・発見・呼び出しを仲介するv1契約を定める。Interpreterは計算を提供する別プロセスまたは別アプリであり、ノード内に動的ライブラリとしてロードしない。遠隔ノード上のInterpreterを探すこと、未起動の提供元を自動起動することはv1の範囲外。

## 1. 主体と権限

- **呼び出し元アプリ**: アプリ資格情報で認証され、`call` grantを持つ。
- **提供元アプリ**: アプリ資格情報で認証され、`provide` grantを持つ。`provide_names`を列挙した場合は、その名前だけを提供できる。Provider APIのWebSocketを自分から開く。
- **所有者**: `manage_apps`を持つ管理資格情報。Provider登録の承認・同名提供元の選択を行う。
- Providerは`call`や`read`を必要としない。逆に、呼び出し元は`provide`を必要としない。
- 呼び出しによってProviderへHarmionのRead/Write権限を渡さない。Providerが受け取るのは、呼び出し元が固定したContentスナップショットと明示入力だけである。
- Provider接続は`/provider/v1/connect`へのloopback HTTPS WebSocket。アプリ資格情報を`Authorization: Bearer`で提示する。ブラウザOrigin付き接続は拒否する。ブラウザで動くProviderは、OAuth専用originの`GET /provider/connect?ticket=...`へ単回・短命のticketで接続する。
- grant変更・失効時、提供元セッションは次の呼び出し前にDBで再検査される。資格情報の失効または`provide`の削除で、そのアプリの確立済みセッションは閉じ、実行中の呼び出しは503または401で終了する。`call`の削除・失効では、そのアプリの実行中の呼び出しを終了する。`provide_names`の縮小でも、許可されなくなったProviderのセッションは閉じ、実行中の呼び出しを503で終了する。`provide`全体の削除・アプリ失効では、そのアプリの全Providerセッションを閉じる。

grantはアプリ単位の`call`と`provide`、および`provide`時の任意の許可名`provide_names`を持つ。`provide_names`が空なら全Provider名を提供できる（OAuthの`provide:all`）。名前を列挙した場合はその名前だけを提供でき、同じ名前のProvider登録は所有者承認を待たずにapprovedになる。`call`/`provide`はRead/Writeを含意しない。grant変更で`provide_names`から外れた登録は同一transactionでsupersededになる。呼び出し回数・課金・監査、遠隔ノードへの委譲はv1に含めない。

## 2. Provider登録

`POST /app/v1/interpreter-register`（`provide`が必要）の本文は次のJSON。

```json
{
  "name": "ledger/balance",
  "contract_version": 1,
  "input_schema": {"type": "object"},
  "output_schema": {"type": "object"},
  "default_query": {"kinds": ["ledger"]}
}
```

- `name`は1〜128バイトのASCII。小文字英数字と`. _ - /`だけを使い、先頭・末尾の`/`と`//`を禁止する。
- `contract_version`はv1では`1`のみ。
- `input_schema`と`output_schema`はJSON object。それぞれ符号化後64 KiB以下。v1のノードはJSON Schemaとして解釈せず、呼び出し元向けの宣言として保存・返却する。
- `default_query`は省略可能。v1では`{"kinds": [...]}`だけを受け付け、1〜32個の重複しないkind名を持つ。ノードはこれを実行しない。呼び出し元が自分のQuery条件とANDで結合し、得たContentのIDと編集トークンを`call`へ渡す。デフォルトQueryによってProviderにデータアクセス権限は生じない。
- `provide_names`が空でない場合、`name`はその集合に含まれなければならない。含まれない登録は403 `forbidden`。`provide`がないアプリの登録も同様。
- 本文全体は160 KiB以下。未知のフィールドは拒否する。

応答は`provider_id`、`name`、`contract_version`、`status`。要求名がアプリの`provide_names`に含まれる場合は`approved`で作成し、`approved_at`を設定する（統合接続でも同じ）。`provide_names`が空（全名前）の場合は`pending`。名前が列挙されていて含まれない場合は403 `forbidden`。同じアプリが同じ名前を再登録すると、`provider_id`と承認状態を保ったままmanifestを更新する。supersededの登録を再登録した場合は`pending`に戻る（現在のgrantが名前を許可していればapproved）。同じ名前を別アプリが登録しても、承認されるまでは共存できる。

## 3. 承認と名前の一意性

`POST /app/v1/interpreter-approve`（`manage_apps`が必要）の本文は次のJSON。

```json
{"provider_id": "hex", "replace": false}
```

- 承認済みの同じ名前のProviderが別に存在し、`replace`がfalseなら409 `interpreter_name_conflict`。trueなら既存をsupersededにし、新しい登録をapprovedにする。
- supersededの登録は承認できない。再登録してpendingに戻す必要がある。
- 承認は冪等。既にapprovedの同じProviderへの承認は成功する。
- `provide_names`の変更・削除では、許可されなくなった登録を同一transactionでsupersededにする。実行中セッションも閉じ、実行中の呼び出しを503で終了する。
- 手動の`interpreter-approve`も、登録元アプリの現在の`provide_names`を再検査する。許可外・アプリ失効・期限切れの登録は403 `interpreter_not_approved`。
- ノード内で承認済みの名前は一つだけ。未承認の同名登録は複数あり得る。
- `interpreter-list`は、認証済みアプリに承認済みProviderを返す。自分の未承認登録と、管理者には未承認も返す。`interpreter-describe`は承認済み、または自分の登録、管理者だけにmanifestを返す。
- 登録・承認はSQLite schema v14の`interpreters`に、`provide_names`はschema v15の`app_provide_names`に、統合接続はschema v16の`connection_requests`に永続化する。再起動後も保持する。承認状態と現在の接続状態は別である。

## 4. Provider接続

### 4.1 ハンドシェイク

`GET /provider/v1/connect`をTLS上で開き、HTTP/1.1 Upgradeを行う。

必須ヘッダー: `Host: harmion.local`、`Authorization: Bearer <app credential>`、`Upgrade: websocket`、`Connection: Upgrade`、`Sec-WebSocket-Version: 13`、`Sec-WebSocket-Key`。`Origin`は禁止。認証失敗は401、Upgrade不正は400。

ブラウザProviderは、App APIの`interpreter-ticket`または統合接続の完了応答で得た`websocket_url`を使う。URLはOAuth専用originの`/provider/connect?ticket=...`で、ticketは30秒・単回使用。ticketはアプリと`provider_id`を束縛し、接続時に現在の`provide`・`provide_names`・承認状態をDBで再検査する。ticketをURL・ログへ長期保存しない。

101応答後、最初のテキストメッセージを10秒以内に送る。

```json
{"jsonrpc":"2.0","id":"attach","method":"interpreter.attach","params":{"provider_id":"hex"}}
```

成功応答:

```json
{"jsonrpc":"2.0","id":"attach","result":{"ok":true,"provider_id":"hex","name":"ledger/balance","contract_version":1}}
```

- 登録がapprovedで、認証されたアプリが登録の所有元で、現在`provide`を持つ場合だけ成功する。
- 拒否はJSON-RPC error。`-32010`は未承認・他アプリ所有、`-32011`は提供元不一致、`-32012`は同じ接続での二重attach。接続は閉じる。
- 一つの`provider_id`に同時に接続できるセッションは一つ。新しいattachは古いセッションを閉じ、古いセッションの実行中呼び出しを503相当で終了させる。
- セッションはプロセス内だけに保持し、ノード再起動で失われる。ノードは20秒ごとにWebSocket Pingを送り、ProviderはPongを返す。Pingの書込み失敗で接続を閉じるが、Pongの期限監視による半切断検出はv1では行わない。
- Rustの`ProviderClient`は1接続を順に扱う。`reconnect`で同じProviderへ再接続でき、`reconnect_with_retry`/`connect_with_retry`はtransport/timeout/429/503だけを指数backoffで再試行する。呼び出しの並列実行ループや、失われたinvocationの自動再送は提供しない。

### 4.2 呼び出し

ノードからProviderへの要求:

```json
{
  "jsonrpc": "2.0",
  "id": "invocation id hex",
  "method": "interpreter.invoke",
  "params": {
    "invocation_id": "hex",
    "provider": "ledger/balance",
    "contract_version": 1,
    "input": {},
    "contents": [
      {"content": {"id": "hex", "body": "text", "attributes": [{"namespace": "...", "key": "...", "value": null}], "kinds": ["ledger"]}, "conflict": false}
    ]
  }
}
```

- `input`は呼び出し元が送ったJSON。null、真偽値、数値、文字列、配列、オブジェクト。深さ128・要素合計32,768・符号化後128 KiB以下。数値はJSONの10進表記を保持する。
- `contents`は呼び出し元が選んだスナップショット。件数上限はない。各Contentは通常値だけで、編集トークン・basis・競合候補・履歴を含まない。`conflict`はその固定basisで通常値が競合しているかを示す。
- この非ストリーミング形式では、本文全体は2 MiB以下（`MAX_PROVIDER_MESSAGE_BYTES`からinvocation envelope分を引いた上限）。ノードは上限超過を呼び出し元へ413で返し、Providerへ送らない。64件を超える入力も、このフレーム上限に収まる限り同じ1回の`interpreter.invoke`で送る。より大きい入力は§4.3のストリーミングを使う。
- ProviderはContent IDから最新値を取り直す契約を持たない。ノードは呼び出し中にQueryやSubscribeを暗黙に実行しない。

成功応答:

```json
{"jsonrpc":"2.0","id":"hex","result":{"status":"ok","data":{},"display":{"media_type":"text/plain","text":"..."}}}
```

- `data`は必須で任意のJSON。符号化後1 MiB以下。
- `display`は省略可能またはnull。`media_type`は128バイト以下のASCIIで、`type/subtype`と`+ - . _`だけを使う（`; charset=...`等のパラメーターはv1では送らない）。`text`は64 KiB以下。表示専用で、逆変換・編集契約はない。
- 失敗応答:

```json
{"jsonrpc":"2.0","id":"hex","result":{"status":"error","error":{"code":"unsupported","message":"...","details":{}}}}
```

- `code`は1〜64バイトの小文字英数字と`. _ -`。`message`は1024バイト以下。`details`は任意JSONで256 KiB以下。ノードはProvider固有のcodeを解釈しない。
- JSON-RPC errorはtransport/protocol違反を表し、アプリケーションエラーには使わない。結果の`status`が不正、または上限超過なら502 `interpreter_invalid_response`。

### 4.3 ストリーミング入力（v1追加契約）

非ストリーミング呼び出しは従来どおり。入力全体が2 MiBを超え得る場合、呼び出し元はApp APIの`interpreter-call-stream`を使う。Provider側は同じ`interpreter.invoke`要求IDで結果を返すため、結果とキャンセルの契約は変わらない。

呼び出し元App APIのフレーム（1フレーム1 HTTP要求、各要求は1 MiB以下）:

```json
{"frame":"start","stream_id":"hex32","provider":"wiki/all","input":{},"timeout_ms":10000}
{"frame":"chunk","stream_id":"hex32","seq":0,"contents":[{"id":"hex32","token":"hex32"}]}
{"frame":"end","stream_id":"hex32"}
{"frame":"cancel","stream_id":"hex32"}
```

- `stream_id`は呼び出し元が生成する32バイトのhex。アプリと有効期間内で一意で、重複は400 `invalid_request`。
- `chunk.seq`は0から始まる連続した番号。順序違反・Content IDのストリーム内重複は状態を変えず、明示エラーを返す。同じ呼び出し元だけがchunk/end/cancelできる。
- 応答は`{"stream_id":"hex","status":"open","limit_bytes":134217728,"expires_in_ms":60000}`、chunk成功は`{"stream_id":"hex","status":"accepted","seq":N,"next_seq":N+1,"accepted_contents":M,"accepted_bytes":B,"limit_bytes":134217728}`、cancelは`{"stream_id":"hex","status":"canceled","canceled":true}`。
- `end`は通常の呼び出し結果（`invocation_id`・`status`・`data`・`display`、または`status:"error"`）に`stream_id`を加えて返す。`timeout_ms`は`end`後のProvider待機時間であり、アップロード期限60秒とは別。
- アップロードの論理上限は128 MiB（`MAX_STREAM_INPUT_BYTES`）。Content選択フレームの受理バイトと、終了時に実体化したProvider Contentの符号化バイトの両方で検査する。超過は`status:"error"`、`error.code:"stream_limit_exceeded"`、`partial:true`、`actual_bytes`/`limit_bytes`/`accepted_contents`/`accepted_bytes`を返し、部分入力は破棄する。無言の切り捨ても部分的な呼び出しも行わない。
- その他のストリームエラーはHTTP 200の`status:"error"`で`stream_not_found`、`stream_expired`、`stream_sequence`、`stream_duplicate_content`。認証・grant・容量・Provider不在は通常のApp APIエラー（401/403/429/503）。不正JSON・未知フィールドは400 `invalid_request`。

Provider側のフレーム（同じWebSocket接続）:

```json
{"jsonrpc":"2.0","id":"invocation hex","method":"interpreter.invoke","params":{
  "invocation_id":"hex","provider":"wiki/all","contract_version":1,"input":{},"contents":[],
  "stream":{"version":1,"chunks":2,"total_contents":65,"total_bytes":123456}
}}
{"jsonrpc":"2.0","method":"interpreter.invoke.chunk","params":{
  "invocation_id":"hex","seq":0,"contents":[...]
}}
{"jsonrpc":"2.0","method":"interpreter.invoke.end","params":{
  "invocation_id":"hex","seq":2,"total_contents":65,"total_bytes":123456
}}
```

- `stream.version`は1。`chunks`はchunk通知数、`total_contents`はContent総数、`total_bytes`は各`ProviderContent`の符号化長の合計。開始要求の`contents`は空配列。
- chunkは最大4096個、`seq`は0から連続。チャンク目標は符号化Content 768 KiBで、各メッセージは2 MiB以下。`end.seq`はchunk数と一致する。
- Providerは`end`まで内容を蓄積してから結果を返す。途中で`interpreter.cancel`を受けた場合は破棄する。Provider SDK `ProviderClient::next`は自動的に蓄積し、完了した`InvokeParams`を返す。
- 非ストリーミングの`interpreter.invoke`は`stream`フィールドを持たない。旧Provider実装はストリーミング呼び出しを受けない限り変更不要。

### 4.4 キャンセル

ノードからProviderへの通知:

```json
{"jsonrpc":"2.0","method":"interpreter.cancel","params":{"invocation_id":"hex"}}
```

Providerは処理を中止するよう努める。応答はない。キャンセル後に届いた同じIDの応答は無視する。Providerがさらに実行を続けても、ノードは呼び出し元へ結果を返さない。純粋性・即時停止はProvider実装の契約であり、v1はサンドボックスで強制しない。

### 4.5 フレーミング

- テキストフレームだけを使う。バイナリフレームは1003で閉じる。
- クライアントからサーバーへのフレームはマスク必須。サーバーからはマスクしない。
- メッセージは1〜2 MiBの上限内で分割できる。制御フレームは125バイト以下で分割しない。
- PingにはPongを返す。CloseではCloseを返す。拡張・サブプロトコルはv1では使わない。
- 無効なフレーム、UTF-8でないテキスト、上限超過はclose 1002/1007/1009で終了する。

## 5. 呼び出し元App API

### 5.1 一覧・説明

- `POST /app/v1/interpreter-list`（本文なし）: `{"providers":[{"provider_id":"hex","name":"...","contract_version":1,"status":"approved","connected":true}]}`
- `POST /app/v1/interpreter-describe`（本文は32バイトのprovider ID）: `provider_id`、`name`、`contract_version`、`status`、`connected`、`input_schema`、`output_schema`、`default_query`。
- `connected`は現在のプロセス内セッションだけを表す。過去の接続・到達性・実行保証ではない。

### 5.2 Provider ticket

`POST /app/v1/interpreter-ticket`（`provide`と承認済みProviderが必要）の本文は32バイトのprovider ID、またはJSON `{"provider_id":"hex"}`。

応答は`ticket`、`expires_in`（秒）、`websocket_url`。OAuth専用originが無効なノードでは400。ブラウザProviderはこのURLへ接続し、`interpreter.attach`を送る。

OAuth originでは、access tokenで同じticketを再発行できる。

`POST /connect/ticket`。ヘッダー`Authorization: Bearer <access token>`、本文`{"provider_id":"hex"}`。CORSはloopback originにだけ許可し、`Authorization`ヘッダーを許可する。ticketは30秒・単回・アプリごと8件・全体128件まで。`connect-complete`の応答を失った場合やWebSocket接続に失敗した場合、再起動後にaccess tokenが残っていれば、このendpointでticketを取り直せる。ticket自体はノードのメモリだけに保持し、ノード再起動では失効する。

### 5.3 呼び出し

`POST /app/v1/interpreter-call`（`call`が必要）の本文:

```json
{
  "provider": "ledger/balance",
  "input": {},
  "contents": [{"id": "hex", "token": "hex"}],
  "timeout_ms": 10000
}
```

- `contents[].token`は`get`/`query`等で得た編集トークン。ノードはトークンが固定したbasisのスナップショットをProviderへ渡す。トークン自体はProviderへ渡さない。同じContentを重複して指定できない。件数上限はなく、1 MiBの本文とProviderの2 MiBフレームに収まる範囲で64件超も指定できる。
- Contentは呼び出し元の現在のRead権限で可視でなければ404。トークン不明・回収済みは410 `token_expired`、Content不一致は400。
- `timeout_ms`は省略時10000、100〜30000。範囲外は400。
- 成功はHTTP 200で`{"invocation_id":"hex","status":"ok","data":...,"display":...}`。
- ProviderアプリケーションエラーはHTTP 200で`{"invocation_id":"hex","status":"error","error":{...}}`。
- Provider未接続・セッション消失は503 `interpreter_unavailable`。タイムアウトは504 `interpreter_timeout`。呼び出し元によるキャンセルは409 `interpreter_cancelled`。同時実行上限は429 `interpreter_capacity`。ノード全体32件、アプリごと8件、Providerごと8件。
- 呼び出し元のHTTPタスクが消えた場合、ノードはcancelを送り、待機状態を回収する。
- 呼び出しは冪等ではない。同じ要求の再送は別のinvocationになり得る。request receiptはv1にない。

### 5.4 ストリーミング呼び出し

`POST /app/v1/interpreter-call-stream`（`call`が必要）へ§4.3の`frame`を1要求ずつ送る。本文・応答・エラーの形は§4.3に定める。

- `start`はProvider名・明示入力・`timeout_ms`だけを送り、Contentはchunkで送る。`start`時点で登録・grant・Provider可用性を検査するが、Contentの実体化は`end`時に行う。
- アップロードは60秒以内。`start`・`chunk`・`cancel`は同じアプリだけが操作できる。別アプリのstream IDは403、未知・期限切れ・終了済みは`stream_not_found`/`stream_expired`。
- 1つのstreamは`end`または`cancel`で単回消費される。`end`後のキャンセルは通常の`interpreter-cancel`（invocation ID）を使う。
- 同時に開けるstreamはノード全体32件・アプリごと8件。超過は429 `interpreter_capacity`。staged入力はプロセス内だけに保持し、ノード再起動で失われる。
- App APIの1要求上限（1 MiB）はフレーム単位で適用される。合計の論理上限は128 MiBで、超過は部分状態付きの明示エラーになる。

### 5.5 キャンセル

`POST /app/v1/interpreter-cancel`（`call`が必要）の本文は32バイトのinvocation ID。

- 呼び出しを開始したアプリだけがキャンセルできる。他人のinvocationは403、未知・完了済みは404。
- 成功は`{"canceled":true}`。待機中の呼び出しは409で終了し、Providerへcancel通知を送る。
- 一時的な識別子は呼び出し元アプリにだけ返す。管理UI・他アプリへ公開しない。

## 6. 統合接続

アプリの初回接続とProvider登録を、所有者の1回の承認にまとめる。アプリは`connect-start`で要求を作り、所有者は管理UIの「接続」画面で承認・拒否し、アプリは`connect-status`・`connect-complete`で結果を受け取る。

### 6.1 開始

`POST /app/v1/connect-start`（App APIでは資格情報不要）またはOAuth専用originの`POST /connect/start`。本文は次のJSON。

```json
{
  "version": 1,
  "name": "AntiWebUI",
  "redirect_uri": "http://127.0.0.1:43123/callback",
  "web_origin": "http://127.0.0.1:43123",
  "client_id": "省略可: 既存OAuthクライアントID hex",
  "challenge": "PKCE S256 challenge 43文字",
  "state": "アプリが生成したstate",
  "grants": {"read": {"type":"all"}, "call": true, "provide": true, "provide_names": ["ledger/balance"]},
  "provider": {"name": "ledger/balance", "contract_version": 1, "input_schema": {}, "output_schema": {}, "default_query": {"kinds": ["ledger"]}},
  "provider_replace": false
}
```

- `version`は1。`name`は1〜128バイト。`redirect_uri`はIPリテラルのloopback HTTP。`web_origin`を指定する場合は`redirect_uri`と同じoriginの`http://127.0.0.1:PORT`または`http://[::1]:PORT`で、パス・クエリを持たない。
- `grants`に`manage_apps`/`manage_nodes`は指定できない。`call`/`provide`はRead/Writeを含意しない。`provide`をtrueにする場合は`provide_names`を1〜32個列挙する。`provider`を指定する場合は名前が`provide_names`に含まれること。
- `provider_replace`はアプリからの置換希望であり、所有者の承認時に明示的な`replace`がなければ置換しない。
- `state`は1〜512バイト、制御文字なし。`challenge`は32バイトのS256。本文は192 KiB以下。未知フィールドは拒否する。
- 応答は`connection_id`、`request_secret`（アプリだけが保持）、`expires_at`、任意で`verification_url`。要求は5分で失効し、同時に32件まで。`request_secret`はURL・ログへ出さない（`verification_url`はフラグメントに含める）。
- 既存`client_id`を指定した場合、登録済みの名前・redirectと一致しなければならない。未指定の場合、承認時にOAuthクライアントを作成する。

### 6.2 所有者の承認

- `POST /app/v1/connect-list`（`manage_apps`）: `{"connections":[...]}`。保留・承認済み・拒否・完了・期限切れを新しい順に最大32件返す。アプリ名、callback、web origin、grant、Provider manifest、状態、期限を含む。`request_secret`は返さない。
- `POST /app/v1/connect-approve`（`manage_apps`）: `{"connection_id":"hex","allow":true|false,"replace":false}`。
  - 承認時、OAuthクライアントの作成（必要時）、grantの確定、Provider登録・承認を1つのDB transactionで行う。`provide_names`にあるProvider名は自動承認される。同名の承認済みProviderが別にあり`replace`がfalseなら409 `interpreter_name_conflict`。`replace`がtrueなら既存をsupersededにし、実行中セッションを閉じる。
  - 拒否時は資格情報もProviderも発行しない。
  - 承認・拒否は冪等ではなく、pending以外は409/410。
- 承認後、アプリは`connect-status`で`approved`を確認する。

### 6.3 状態と完了

- `POST /app/v1/connect-status`（App APIでは資格情報不要）またはOAuth origin`POST /connect/status`。本文は`{"connection_id":"hex","request_secret":"hex"}`。応答は`connection_id`、`status`（`pending`/`approved`/`denied`/`completed`/`expired`）、`name`、`web_origin`、`provider_name`、`expires_at`。secret不一致は404。
- `POST /app/v1/connect-complete`またはOAuth origin`POST /connect/complete`。本文は`{"connection_id":"hex","request_secret":"hex","code_verifier":"PKCE verifier"}`。
  - 承認済みでsecretとPKCEが一致した場合だけ、access token（10分）、refresh token（30日系列）、scope、`client_id`、任意のProvider `ticket`/`websocket_url`を返す。完了は単回で、応答を失った場合は要求を作り直す。
  - 未承認は409 `connection_pending`、拒否は403 `connection_denied`、期限切れは410 `connection_expired`、完了済みは409 `connection_completed`。secret不一致は404。
  - 応答のscopeは承認時のgrantを表す。App APIの認証時はDBの現在grantを再検査するため、以後の縮小・失効は既存access tokenにも及ぶ。
  - 同じ`connection_id`・request secret・PKCE verifierの完了要求は、成功後60秒間プロセス内キャッシュから同じ応答を返す。ノード再起動後はキャッシュが消えるため409 `connection_completed`になる。

### 6.4 Webアプリの完了通知

OAuth専用originは`GET /connect`に小さなヘルパーページを返す。Webアプリは`verification_url`をポップアップで開く。ページはフラグメントの`connection_id`・`request_secret`・`state`を使い、`/connect/status`をポーリングする。承認・拒否・期限切れになると、`web_origin`をtargetOriginにして次を`window.opener.postMessage`する。

```json
{"type":"harmion-connect","status":"approved","connection_id":"hex","state":"..."}
```

- ページは`request_secret`やトークンをopenerへ送らない。アプリはメッセージの`event.origin`がOAuth origin、`state`が自分で生成した値であることを確認し、自分で`connect-complete`を呼ぶ。
- ポップアップが使えない場合は、アプリが直接`connect-status`をポーリングしてもよい。
- `connect-start`・`connect-status`・`connect-complete`・`connect-ticket`はCORSを許可する。Originはloopbackのhttp/httpsだけ。`connect-ticket`だけは`Authorization: Bearer`を受け付け、他はCookie・資格情報を受け付けない。

## 7. エラーと限界

App APIのエラー:

| HTTP | code | 意味 |
|---|---|---|
| 400 | `invalid_request` | 形式・長さ・timeout・未知フィールドが不正 |
| 401 | `unauthenticated` | 資格情報が不明・期限切れ・失効 |
| 403 | `forbidden` | grant不足、他アプリの操作 |
| 404 | `not_found` | Provider・Content・invocationがない、または可視でない |
| 409 | `interpreter_name_conflict` | 同名の承認済みProviderが存在 |
| 409 | `interpreter_cancelled` | 呼び出し元がキャンセルした |
| 409 | `connection_pending` | 統合接続が未承認 |
| 409 | `connection_completed` | 統合接続が完了済み |
| 403 | `connection_denied` | 統合接続が拒否された |
| 404 | `connection_not_found` | 統合接続ID・secretがない |
| 410 | `connection_expired` | 統合接続が期限切れ |
| 429 | `connection_capacity` | 保留中の統合接続が上限 |
| 410 | `token_expired` | 入力トークンが回収済み・不明 |
| 413 | `interpreter_input_too_large` | Content入力が上限超過 |
| 429 | `interpreter_capacity` | 同時呼び出し上限 |
| 502 | `interpreter_invalid_response` | Providerの応答が契約違反 |
| 503 | `interpreter_unavailable` | Provider未接続・利用不能 |
| 504 | `interpreter_timeout` | 期限内の応答なし |

ストリーミング入力では、上記に加えてHTTP 200の`status:"error"`本文で`stream_limit_exceeded`・`stream_not_found`・`stream_expired`・`stream_sequence`・`stream_duplicate_content`を返す。境界超過は無言に切り捨てず、`partial:true`と受理済みバイト数を含める。

v1の範囲外: 遠隔Interpreter、自動起動、出力のストリーミング/部分応答、実行サンドボックス、Providerごとのgrant・quota・監査、入力Contentのストリーム取得（呼び出し元はIDと編集トークンを送る）、表示の逆変換、管理UIからの呼び出し補助。これらを実装済みとして扱わない。
