# App保存要求とローカル状態 v1

この段階はnode/applicationの論理契約と、再送要求の同一性を判定する形式。HTTPのendpoint・一般アプリ向けOAuthフローは未接続。

## 保存要求

`SaveRequest` はrequest_id・content_id・commandを持つ。IDは32 bytes。Content IDはクライアントが生成し、同IDの別createは拒否する。更新IDと編集時刻はhostが発行し、要求には含めない。

| command | 入力 | 条件 |
|---|---|---|
| Create | 項目→値 | body必須。作成後のWrite Filter |
| Edit | 編集トークン、項目→値 | 固定された通常値グループのみ置き換える |
| Resolve | 解決トークン、項目→値と明示した版ID | トークン取得時の候補の部分集合のみ |
| Delete | トークン | 通常削除はWrite。削除競合の解決は管理権限 |
| Keep | 管理トークン | 全権限の管理ツール、live/deleted混在basis |
| Restore | 管理トークン、復元全体の項目→値 | 全権限の管理ツール、deletedのみのbasis |

Editの省略項目は未変更。Restoreの省略した既知属性は不存在、既知kindはfalse。非公開kind競合の変更・解決は管理ツール限定。本文の通常編集はそれらを暗黙に解決しない。

## 要求のcanonical bytes（HRQ-v1）

保存再送の比較に使う。record-v1の共通整数・文字列・field・value・ID集合の符号化と同じ。上限も1 MiB・深さ64・要素予算32,768を共通で適用する。

1. magic `48 52 51 00`（HRQ＋NUL）、u32 version=1。
2. request_idとcontent_id、それぞれ32 bytes。
3. u8 command: Create=0、Edit=1、Resolve=2、Delete=3、Keep=4、Restore=5。
4. Create以外は32 bytesの編集/解決/管理トークン。
5. Create/Edit/Restoreはu32個数＋field/value列。Resolveはu32個数＋field/value/replaces列。Delete/Keepは追加データなし。

重複・非canonical順序・未知version/tag・末尾bytesは拒否する。生成済み更新ID・時刻・認証credentialを要求比較に混ぜない。request_idはapp_idと組にして保存し、同じappの別要求への使い回しはRequestIdReuseとする。

## 認可・トークン・再送結果

- 認証は各use caseのSQLiteトランザクション内で行い、現在の失効・期限・grantを確認する。認証コンテキストを外部から組み立てる公開APIはない。
- 初回のOS所有者向けbootstrapだけが最初の管理grantを発行する。既存の失効済みappがあっても再bootstrapは不可。通常のgrant発行・変更・失効はManageAppsを検査する。
- 現段階のcredentialはhostが生成する32 bytesの不透明秘密。保存するのは用途分離したSHA-256 verifierのみ。Debug表示からcredentialと編集トークンを除外する。OAuthの短命access token/refresh rotationは次段階。
- Read/Write FilterはAllまたはKinds（指定kindのいずれかに所属）。全Read/Writeだけでは管理権限にならない。
- 編集トークンはcontent_id・固定basis・用途をDBに保存し、履歴から当時の候補を再構築する。使い捨てにせず、現在basisへ差し替えない。ReadなしのWriteへ別途トークンを渡せるが、トークン自体は権限を与えない。
- 保存前後の通常kindsで現在のWriteを確認する。削除済みContentの古いlive basisからの編集も、保持された現在の項目で認可する。
- 保存成功時は更新、受理ログ、結果用トークン、再送結果を一緒にcommitする。
- 再送は元のupdate_idと保存時snapshotを返す。本文は保存時に返せた場合のみ候補とし、さらに現在のReadが保存時と現在のkindsの両方を許可する場合に限る。失効・期限切れなら再送も拒否。Writeのみの元受領結果に、後から本文を付け足さない。
- 一般ReadViewはContent、不透明トークン、公開可能な項目競合、削除競合フラグのみ。内部basisや非公開kind候補は含めない。管理inspectは削除済みContentと復元用トークンを取得できる。

## SQLite schema v2

v1の正本・ログ・Peerカーソルに、apps、app_kinds、edit_tokens、token_basis、request_resultsを追加。既存のnode/log epochは変えない。これらのローカル表はPeer転送の対象外。保存adapterは認可ルールを持たず、nodeのトランザクション内ユースケースが判断する。

ネットワーク境界の認証・OS側credential保護・OAuth・購読失効は未実装。この契約だけでネットワーク公開可能とは扱わない。
