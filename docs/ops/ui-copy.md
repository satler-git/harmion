# UIコピー監査（調査のみ）

最終更新: 2026-11-27
基準: `92ff053`（`fix(android): apply auto-sync changes without an app restart`）と、その時点の作業ツリー。
監査時点ではアプリコード・文言・リソースを変更していない。実装は末尾「12. 実装状況」に記載した。監査の推奨は「削除 / ヘルプへ移動 / 操作で代替 / 維持」の判断のみで、短縮リライトは行っていない。

> **実装状況（2026-11-27、本コミット）**: Harmion Android・Mote・管理Web UI の P0削除と主要な用語統一は実装済み。CLI と Wiki GUI は未着手。詳細は末尾「12. 実装状況」。
> **用語方針の改訂**: 本文の日本語化推奨より、末尾 12.0 の「英語の製品語を優先」が優先する（`Inbox`/`All`、`short capture`/`long capture`、`peer`、`active`、`Content`、`Export`）。

対象:

| サーフェス | 主なファイル |
|---|---|
| Harmion Androidノード | `android/app/src/main/java/org/harmion/android/{ui,approval,widget,service}/`、`res/layout/activity_harmion_scanner.xml`、`res/values/strings.xml` |
| Mote Android | `android/mote/src/main/java/org/harmion/mote/` |
| 管理Web UI | `apps/admin/ui/src/`、`apps/admin/ui/index.html` |
| native CLI / daemon / admin | `apps/cli/`、`apps/daemon/`、`apps/admin/`、`scripts/harmion-update` |
| Wiki GUI | `apps/wiki-gui/src/` |

除外: コードコメント、ログ、JSONキー、プロトコル値、テスト、デバッグ専用ソース、`out/`・`build/`。clap自動生成の `Usage:` なども除外した。

方法: 上記ソースを通読し、`Text` / `label` / `title` / `placeholder` / `contentDescription` / `aria-label` / `alt` / `title` / `println!` / `eprintln!` / shebangスクリプトの出力を抽出。サブエージェント5系統で並行監査し、本レポートで統合・補正した。実行時の画面確認はしていないため、重なり・折返し・読み上げ順は未検証。

---

## 1. 結論

- 確認したユーザー可視文字列は**約1,300**（重複・集約により±1割）。うち**削除候補 約180**、**ヘルプ移動候補 約130**（ただしCLIの86件は既に `--help` にあり移動不要。実移動候補は約40）、**操作で代替 約25**、残りは維持。
- 最大の問題は個々の文言ではなく、**「常設の説明」と「同じ状態の多重表示」**。AGENTS.md の「字幕・導入段落を既定で置かない」「実装詳細を出さない」に、Android・管理UI・Wikiの各所が直接抵触している。
- 2番目に大きいのは**生の実装エラーの露出**。Harmion の `lastError`、管理UIのブラウザ例外、Wiki GUI の `nvim`/`niri` stderr、CLI の `{self:?}` が該当する。これは削除ではなくマッピング/ラップが必要で、本監査では「維持（要修正）」とした。
- 3番目は**用語と言語の不統一**。`Peer`/`ピア`、`Content`/`コンテンツ`、`Export`/`エクスポート`、`Inbox`/`All`、`参加コード`/`招待コード`/`参加要求コード`、`承認済み`/`稼働中` など。統一は「書き換え」になるため、本監査では削除推奨に含めず「要判断」に分離した。
- 削除時に**アクセシビリティを壊す箇所**がある。タブのアイコン contentDescription とラベルの二重読み上げ、Mote 入力欄の `contentDescription = placeholder`、進捗ラベルを消した際の `stateDescription` 不在、通知の操作名など。削除は単独で行わず、セマンティクス調整とセットにする。

### 確度の高い削除トップ10

| # | 対象 | 場所 |
|---|---|---|
| 1 | タブ字幕4件（`ノードと同期の状態` / `端末をつなぐ` / `接続と権限` / `同期と所有者`） | `ui/MainActivity.kt:200-203` |
| 2 | 管理UIの英語eyebrow 11件（`REMOTE ACCESS` / `OWNER ACCESS` / `APPLICATION ACCESS` ほか） | `App.svelte:147,171`、各 `pages/*.svelte` |
| 3 | 管理UIのページ導入段落8件（h1直下の説明文） | `pages/{Apps,Conflicts,Connections,Contents,Export,Interpreters,Overview,Peers}.svelte` |
| 4 | Home の `自動同期はオフです`（ヒーローの `手動同期のみ` と重複） | `ui/Home.kt:238` |
| 5 | Home の `参加の承認待ち`＋`既存ノードの所有者に承認を依頼してください。`（`PendingJoinCard` と重複） | `ui/Home.kt:268-270` |
| 6 | Home の `現在` 行とヒーローの `最終 …`（同期カードの `最終成功` と重複） | `ui/Home.kt:96,286` |
| 7 | Peers の `更新`（右上の再読み込みアイコンと同じ `reload`） | `ui/Peers.kt:360` |
| 8 | Peers の同期エラー見出し（直下本文が同じ文言を含む） | `ui/Peers.kt:457` |
| 9 | Apps の件数字幕と `承認待ち`/`承認済み` ピル（直下のセクション見出しと重複） | `ui/Apps.kt:109,200,325` |
| 10 | Mote の `操作の反応`（トグル1個を包む抽象見出し） | `MoteSettingsScreen.kt:113` |

次点: 管理UIフッター4種、`管理コンソール`、`WORKSPACE / …`、Wiki の `Wiki へようこそ`＋空状態装飾、`閲覧ビュー · 保存は Neovim`、`プレースホルダ`、CLI の `--ui-dir` 3重複。

---

## 2. 分類別件数（概算）

| サーフェス | 合計 | Essential | Redundant | Interaction | Contextual help |
|---|---:|---:|---:|---:|---:|
| Harmion Android | 253 | 192 | 40 | 13 | 8 |
| Mote | 214 | 187 | 19 | 5 | 3 |
| 管理Web UI | 493 | 407 | 61 | 1 | 24 |
| native CLI | 149 | 57 | 5 | 1 | 86 |
| Wiki GUI | 215 | 152 | 52 | 4 | 7 |
| **合計** | **約1,324** | **約995** | **約177** | **約24** | **約128** |

注:
- 文字列は「1つの意味単位」で数えた。`キャンセル` など機械的反復は1件に集約し、出現回数を別記した。
- CLI の Contextual help 86件は `--help` に置くべき説明であり、「常設から外す」対象ではない。実移動候補は、管理UI・Android・Mote・Wiki の約42件。
- 管理UIはラベル群を1行にまとめたため、表の行数は件数より少ない。
- Wiki GUI は `main.rs` と分割ファイルで同一文字列が重複するため、実際の出現は約240。

---

## 3. 横断所見

### 3.1 常設の説明（字幕・eyebrow・導入段落）

- Harmion: タブ字幕4、`SectionHeader` の字幕、Approval の確認サブタイトル、About の `Androidノード`。
- 管理UI: 10画面・11件の英語eyebrow、8画面の導入段落、`管理コンソール`、`WORKSPACE / …`、フッター。
- Mote: `操作の反応`、`Harmion のメモ`。
- Wiki: `ノート`、`Wiki へようこそ`、`ライブラリ`、サイドバーの `ページ`/`最近のページ`、Inspector の空説明。

これらは「直下のUIで自明」か「設計意図の説明」であり、削除しても操作・理解は落ちない。ただし初回ユーザー向けの価値提案（`承認した端末とだけ同期します。`）は、削除ではなくヘルプ/初回のみ表示に移す判断が必要。

### 3.2 同じ状態の多重表示

| 情報 | 重複している場所 |
|---|---|
| 同期状態 | Home ヒーロー（`手動同期のみ`/`同期を待機中`/`待機中`）、同期カードの `現在`、ウィジェット、常駐通知、KeepAlive |
| 最終同期 | Home ヒーロー本文の `最終 …` と同期カードの `最終成功` |
| 自動同期オフ | Home ヒーローの `手動同期のみ` と警告カード `自動同期はオフです` |
| 参加承認待ち | Onboarding のQRカード、Home の承認待ちカード、`PendingJoinCard`、所属ピル |
| 退役確認 | Peers のセクション見出し、カード見出し、メンバーカードの `退役の確認待ち`、詳細シートの `退役の確認` |
| 承認待ち件数 | Apps の字幕、セクション見出し、ピル。管理UIのタブbadge、callout。 |
| 接続状態 | Mote のヘッダーアイコン、チップ、全画面状態、空リスト文言、設定、共有画面、長文画面 |
| 承認モード | 管理UIのヘッダーbadge、tooltip、Overviewの値、Connectionsのcallout、format.ts |
| 2-hop | Wiki の `2-hop リンク`、`次に読む`、`2-hop` chip、ショートカット一覧 |

### 3.3 生の実装エラー・内部語

- Harmion `Home.kt:290` は `snapshot.lastError` をそのまま表示。値は `crates/runtime/src/scheduler.rs:434` の `format!("{}: {code}", hex(&node.0))` や `"sync task failed"` で、ユーザー語ではない。
- `Format.errorLabel` の最終分岐は未知の例外メッセージを raw 返却（`Format.kt:148`）。Home 以外では `it.message` 直表示も残る。
- 保存異常の通知本文は `storage.take(120)` の生エラー（`IssueCheckWorker.kt:76`）。
- Mote はコード別マッピングが比較的整っているが、`Capture`/`Kind`/`照合`/`短文` などの内部語が混ざる（`MoteViewModel.kt:636,674,1001-1002`）。
- 管理UI: `Overview.svelte:178` が `approval.mode` の生enum（`terminal`/`session`/`deny`/`stdio`）を表示。`errorMessage()` はブラウザ例外をそのまま出す。`Grant JSON` など開発者語も常設。
- Wiki: `nvim --server failed`、`niri msg: …`、`Neovide refused the buffer`、`解釈に失敗: {serde}`、`workspace`/`items` がUIに出る。
- CLI: `harmion: InvalidHex` / `harmion: Http(404)` は `crates/client/src/lib.rs:55` と `crates/protocol/src/record.rs:12` の `{self:?}` に起因。daemon/admin は失敗が `ERROR … error=No such file or directory (os error 2)` の1行だけで、対象も対処も出ない。

### 3.4 用語・言語の不統一（削除ではなく要判断）

- Android: `参加コード` / `招待コード` / `参加要求コード`。`承認済み`（membership）と `稼働中`（member state）。`退役` と `退役済み`。`ピア` と `台`。`同期を待機中` と `同期待機`。
- Mote: `Inbox` / `All` / `戻す` / `Inbox に戻す`。`ピン` / `ピン留め` / `ピン留め解除`。`メモ` / `短文` / `Capture`。
- 管理UI: `Peer` / `Interpreter` / `Export`（ナビは `エクスポート`）/ `Content` / `コンテンツ`。進行表示が `更新中…` `読み込み中…` `取得中…` `確認中…` `生成中…` `保存中…` の6系統。
- CLI: helpとエラーは英語、承認プロンプトとsummaryは日本語。summary内に `Export` `credential` `enforcement` `grant` `replace`。
- Wiki: `Neovim` / `Neovide`、`2-hop` / `次に読む`、`発リンク`（非標準語）、`Source::Fallback` が `内蔵サンプル` / `オフラインキャッシュ` / `内蔵 fixture`。

### 3.5 削除すると壊れるもの（維持）

- 破壊的操作の警告・不可逆性・対象の明示（退役、削除、失効、リカバリコード再表示不可）。
- 権限・セキュリティの同意情報（要求権限、強い権限、未検証データ、パッケージ/署名）。
- エラーと復旧手順（`再試行`、`再接続`、`承認をリクエスト`、`照合して採用`）。
- 非自明なクロスデバイス手順（QRを既存ノードで読む、端末の `y` 承認）。
- アクセシビリティラベル（`戻る`、`閉じる`、`ライト`、`ピアの詳細を表示`、QR alt）。
- オフライン時のデータ保全の保証（Mote の「本文は端末に残る」系）。削るなら別の保証表示が必要。
- 空状態（Mote の `Inbox は空です` など）。空白画面を無反応に見せないため、視覚コピーを消すならセマンティクスで補う。

### 3.6 死んでいるコピー

- `res/values/strings.xml` の `node_channel_name` / `sync_running` / `approval_running` は参照なし。
- Mote `quietSuccessMessages` の5件（`Harmion に保存しました。` など）は表示されない。ViewModel 側の対応メッセージも同様。
- Wiki の `今月の残高` / `ledger/balance` / `未取得` / `save の解釈に失敗` は未到達。
- Mote の `元に戻す` actionLabel（`MoteScreen.kt:417`）はカスタム Snackbar に上書きされ未使用。

### 3.7 操作で代替できるもの

- 進捗文言（`同期しています…`、`退役中…`、`確定中…`、`承認中…`、`登録中…`、`復元中…`、`発行中…`、`処理中…`）はスピナー＋disabled で代替可能。ただし TalkBack 用の progress semantics が前提。
- 同じ `reload` に対するアイコンと `更新` ボタン（Peers）。
- コピー完了のインライン表示と Toast の二重フィードバック（Settings）。
- Home の同期ボタンは既に「実行→進捗→成功」を1つのボタンで表現している。`同期しています…` の重複をさらに足す必要はない。
- Mote の `Inbox`/`All` は2状態セグメントではなく、アーカイブ表示のトグルにできる（要UI変更）。
- Wiki の `クリックで閉じる ·` / `クリックで表示 ·` はシェブロンと開閉状態で代替。
- CLI の `--restart` 注意書きは、`--restart` 使用時/未使用時に条件分岐できる。

### 3.8 ヘルプへ移すもの

- Approval の「なぜパッケージと署名を見るのか」の詳細説明。
- Apps の「権限変更には所有者承認が必要」、`所有者の生体認証で確定します`。
- Conflicts の `3-wayマージ`。
- Mote の未確定結果の調停プロトコル説明、`下書きを削除…` と `端末の記録を破棄…` の違い。
- 管理UIの承認モード（`R2`/`TTY`/`--approval`）、Interpreters の「承認と接続の違い」、Provider ヒント。
- Wiki のショートカット一覧（既に `?` がある）、`発リンク` などの用語。
- CLI の上級フラグ説明（`--approval`、`reset-peer-v2` の実装理由）。CLI は `--help` 自体がヘルプ面なので、常設ログから `--help` へ移す。

---

## 4. Harmion Android

### 4.1 ナビゲーション

| 文字列 | 場所 | 分類 | 判断 |
|---|---|---|---|
| `ノードと同期の状態` / `端末をつなぐ` / `接続と権限` / `同期と所有者` | `MainActivity.kt:200-203` | Redundant | 削除。タブ名と画面内容の言い換え |
| `contentDescription = item.title`（タブアイコン） | `MainActivity.kt:231` | Redundant | `null` にする。直下のラベルと二重読み上げ |

### 4.2 Onboarding

| 文字列 | 場所 | 分類 | 判断 |
|---|---|---|---|
| `承認した端末とだけ同期します。` | `Onboarding.kt:113` | Contextual help | 初回のみ/ヘルプへ。信頼モデルの説明自体は保持 |
| `処理しています…` | `Onboarding.kt:214` | Interaction | スピナー＋progress semantics へ |
| `harmion-peer:v1:…` | `Onboarding.kt:230` | Redundant | 削除。ワイヤ形式の露出 |
| `参加承認待ち` ×2 | `Onboarding.kt:128`、`PendingJoin.kt:129` | Redundant | 1箇所に統合（Home の membership カードも含む） |
| `既存ノードの「参加要求を承認」でこのQRを読み取ってください。` / 同 `読み取る` | `Onboarding.kt:130`、`PendingJoin.kt:131` | Essential | 文言は統合。手順自体は保持 |
| `参加コードを共有` ×2 | `Onboarding.kt:152`、`PendingJoin.kt:172` | Essential | 共有シートの文脈。保持（重複実装は統合） |
| `参加要求コード` ×2 | `Onboarding.kt:140`、`PendingJoin.kt:161` | Essential | 画像の代替テキスト。保持 |
| `この端末だけで始める` / `既存のクラスタに参加` | `Onboarding.kt:168,193` | Essential | 2択の識別。保持 |

`Harmion`（`Onboarding.kt:108`）は初回画面でアプリを識別する唯一の見出しなので**維持**とする（サブエージェントの削除案は採らない）。`参加コード` / `招待コード` / `参加要求コード` の統一は要判断。

### 4.3 Home

| 文字列 | 場所 | 分類 | 判断 |
|---|---|---|---|
| `アプリを開くと再開します` | `Home.kt:96` | Redundant | 削除。開いている画面で矛盾し、再開は自動 |
| `最終 {相対時刻}（HH:mm）`（ヒーロー本文） | `Home.kt:99` | Redundant | 同期カードの `最終成功` とどちらか一方に統合 |
| 停止理由（ヒーロー本文） | `Home.kt:97` | Redundant | `自動同期を停止中` カードと同時表示。片方に統合 |
| `自動同期はオフです` | `Home.kt:238` | Redundant | 削除。ヒーローの `手動同期のみ` と重複 |
| `参加の承認待ち` / `既存ノードの所有者に承認を依頼してください。` | `Home.kt:268-270` | Redundant | 削除。`PendingJoinCard` と所属ピルに統合 |
| `現在` 行 | `Home.kt:282` | Redundant | 削除。ヒーローの状態と重複 |
| `SectionHeader("ノード")` | `Home.kt:323` | Redundant | 削除。直下の `ノードID` で自明 |
| `同期しています…`（ボタン） | `Home.kt:427` | Interaction | スピナー＋`stateDescription` へ。ボタンの `同期しました` は結果表示として保持 |
| `snapshot.lastError` の直表示 | `Home.kt:290` | Essential | **維持（要修正）**。`errorLabel` 経由のユーザー語へ。今は `<hex>: code` が漏れる |
| `同期` / `再試行待ち` / `最終成功` / `今すぐ同期` | `Home.kt:281-298,420-428` | Essential | 保持 |

### 4.4 Peers

| 文字列 | 場所 | 分類 | 判断 |
|---|---|---|---|
| `更新` | `Peers.kt:360` | Redundant | 削除。右上の `再読み込み` アイコンと同じ `reload` |
| `同期記録を取得できませんでした`（見出し） | `Peers.kt:457` | Redundant | 削除。本文（`:179`）が同じ文言を含む |
| `{n} 件の判断が必要です` | `Peers.kt:468` | Redundant | 削除。カード枚数で足りる |
| `{n} 台のノード` | `Peers.kt:497` | Redundant | 削除。一覧から数えられる |
| `招待コードを読み取ると参加できます。` | `Peers.kt:506` | Redundant | 削除。招待QRと重複し、アクティブノードでは読み取り側でない |
| `状態` InfoRow | `Peers.kt:959` | Redundant | 削除。上部の StatusPill と重複 |
| `相手が確認済みの位置` / `相手の最新位置` | `Peers.kt:979-983` | Contextual help | 詳細シート内なので既に段階開示。削るならヘルプへ |
| `招待コードを共有`（chooser title） | `Peers.kt:347` | Redundant | 削除。共有シートの内容で自明 |
| `退役中…` / `確定中…` / `処理中…` / `承認中…` | `Peers.kt:598,666,713,785,1064` | Interaction | スピナー＋disabled へ。ただし progress semantics を付ける |
| `ピアを退役しました。` / `退役を確定しました。` / `退役を却下しました。` | `Peers.kt:245,284` | Essential | 破壊的操作の結果通知として保持 |
| `この操作は取り消せません。` / `この判断は取り消せません。` | `Peers.kt:571,637,691` | Essential | 保持 |
| `この端末を退役させますか？` / `同じノードIDでは再参加できません。` | `Peers.kt:619,630` | Essential | 自己退役の重大警告。保持 |
| `ピアの詳細` / `ノードID` / `ノードIDをコピー` | `Peers.kt:921,935,954` | Essential | シートの識別とコピー。保持 |
| `発行元 X → 対象 Y` | `Peers.kt:1044` | Essential | 照合情報。保持 |

注: `参加要求を承認`（`Peers.kt:389`）とダイアログ `参加を承認`（`:731`）、`ピアの詳細` と `状態` の粒度は不統一。削除ではなく統一の判断が必要。

### 4.5 Apps

| 文字列 | 場所 | 分類 | 判断 |
|---|---|---|---|
| `承認待ち {n} 件 / 承認済み {n} 件` | `Apps.kt:109` | Redundant | 削除。直下の `承認待ち`/`承認済み` セクションと重複 |
| `承認待ち` ピル | `Apps.kt:325` | Redundant | 削除。セクション見出しと重複 |
| `承認済み` ピル | `Apps.kt:200` | Redundant | 削除。セクション見出しと重複 |
| `接続要求がここに表示されます。` | `Apps.kt:215` | Redundant | 削除。空状態の見出しで足りる |
| `権限の変更は所有者承認が必要です` | `Apps.kt:165` | Contextual help | 常設字幕をやめ、管理操作時/ヘルプへ |
| `所有者の生体認証で確定します` | `Apps.kt:436` | Contextual help | 生体認証プロンプト自体で自明。ヘルプへ |
| `強い権限を要求しています` | `Apps.kt:340` | Essential | 保持。権限判断の警告 |
| `アプリ`（名前欠落時のフォールバック） | `Apps.kt:191,316,380,432` | Essential | 保持 |

### 4.6 Settings

| 文字列 | 場所 | 分類 | 判断 |
|---|---|---|---|
| `アプリ`＋バージョン | `Settings.kt:241` | Redundant | About の `アプリ` と重複。どちらか一方へ |
| `通知に状態を表示` | `Settings.kt:221` | Essential | 常駐通知の副作用説明。保持 |
| `所有者の状態を確認しています…` | `Settings.kt:395` | Interaction | スピナー＋progress semantics へ |
| `ノードの所有者キーを復元します。` | `Settings.kt:500` | Redundant | 削除。ダイアログタイトルと `復元` ボタンで自明 |
| `コピーしました`（インライン） | `Settings.kt:674` | Redundant | Toast `リカバリコードをコピーしました`（`:689`）と二重。どちらか一方 |
| `登録中…` / `復元中…` / `発行中…` | `Settings.kt:486,566,619` | Interaction | スピナー＋disabled へ。a11y の progress を付ける |
| `適用` | `Settings.kt:343` | Essential | 何の適用か不明瞭。削除ではなく用語統一の判断 |

`現在の所有者キーは失効します。` / `現在のリカバリコードは無効になります。` / `このコードは再表示できません。` は保持。

### 4.7 接続承認（ApprovalActivity）

| 文字列 | 場所 | 分類 | 判断 |
|---|---|---|---|
| `Android が確認したパッケージと署名だけを表示します` | `ApprovalActivity.kt:476` | Redundant | 削除。同カードの警告（`:491`）と重複 |
| `この時刻を過ぎると、この承認は失効します` | `ApprovalActivity.kt:670` | Redundant | 削除。`要求の有効期限` で自明 |
| `許可すると、このアプリは上記の権限でノードにアクセスできます。` | `ApprovalActivity.kt:737` | Redundant | 削除。権限一覧＋許可ボタンで自明 |
| `権限情報を取得できないため許可できません。` | `ApprovalActivity.kt:739` | Redundant | 削除。`:570` の警告＋disabled と重複 |
| `処理中…` | `ApprovalActivity.kt:732` | Interaction | スピナー＋disabled へ |
| `この画面のタイトルとアイコンはAndroidが確認したアプリです。下のパッケージと署名を優先してください。` | `ApprovalActivity.kt:491` | Essential | なりすまし対策。保持（ヘルプ併用は可） |
| `要求元が指定した情報` / `未検証の表示です。…` | `ApprovalActivity.kt:520-521` | Essential | 未検証データの警告。保持 |

### 4.8 競合解決（Conflicts）

| 文字列 | 場所 | 分類 | 判断 |
|---|---|---|---|
| `競合`（シート見出し） | `Conflicts.kt:125` | Redundant | 削除。カードの `競合 N 件` と重複 |
| `競合を解決`（詳細見出し） | `Conflicts.kt:190` | Redundant | 削除。`戻る`＋内容で自明 |
| `項目を選択してください` | `Conflicts.kt:378` | Redundant | 削除。ボタン disabled で到達不能 |
| `3-wayマージ` | `Conflicts.kt:284` | Contextual help | 手法名はヘルプへ。ボタンは操作として残す |
| `この内容で解決` / `選択した内容で解決` | `Conflicts.kt:341,391` | Interaction | 同時に主操作が2つ。状態に応じて1つに |

### 4.9 About

| 文字列 | 場所 | 分類 | 判断 |
|---|---|---|---|
| `Androidノード` | `AboutActivity.kt:107` | Redundant | 削除。タグライン |
| `リンク` 見出し | `AboutActivity.kt:141` | Redundant | 削除。直下の GitHub ボタン1つで自明 |
| `種別` / `ビルド日時` / `ブランチ` / `パッケージ` | `AboutActivity.kt:120-135` | Contextual help | サポート情報。詳細/コピー用のヘルプへ |
| `バージョン` / `アプリ` / `ビルド` / `コミット` / `GitHub` / ライセンス | `AboutActivity.kt:117-151` | Essential | 保持 |

### 4.10 ウィジェット・通知・スキャナ

| 文字列 | 場所 | 分類 | 判断 |
|---|---|---|---|
| `HARMION` | `activity_harmion_scanner.xml:58` | Redundant | 削除。装飾 |
| `storage.take(120)`（通知本文） | `IssueCheckWorker.kt:76` | Contextual help | 生エラーを通知本文から外し、詳細画面/ヘルプへ |
| `ノード通知` / `同期しています` / `承認を待っています` | `strings.xml:4,7,8` | Redundant | 未使用リソース。削除 |
| `タップして解決` | `IssueCheckWorker.kt:71` | Essential | 通知からの操作説明。保持 |
| `Harmion` / 状態 / `N ピア` / `開く` / `同期` | `HarmionWidget.kt` | Essential | 保持 |

---

## 5. Mote

| 文字列 | 場所 | 分類 | 判断 |
|---|---|---|---|
| `Mote`（ヘッダー） | `MoteScreen.kt:768` | Redundant | 単一画面アプリの画面名。削除候補（低確度） |
| `この端末で下書きを続けられます。` | `MoteScreen.kt:952` | Contextual help | オフライン時の補足。ヘルプへ |
| `入力した下書きはこの端末に残ります。` | `MoteScreen.kt:961` | Contextual help | データ保全の説明。ヘルプへ（消すなら別の保証表示） |
| `下書きはこの端末で編集できます。` | `LongCaptureScreen.kt:324` | Contextual help | 編集可能な入力欄で自明。ただしオフライン保証として残す判断も可 |
| `再開` | `MoteScreen.kt:1144` | Interaction | カード全体タップ＋`長文を再開` a11y で代替 |
| `Inbox` / `All` | `MoteScreen.kt:1160` | Interaction | 英語の内部概念。アーカイブ表示トグルへ（要UI変更） |
| `操作の反応` | `MoteSettingsScreen.kt:113` | Redundant | 削除。トグル1個の抽象見出し |
| `Harmion のメモ` | `MoteSettingsScreen.kt:171` | Redundant | 削除。タグライン |
| `種別` / `ビルド日時` / `パッケージ` / `リビジョン` | `MoteSettingsScreen.kt:177-183` | Contextual help | サポート用の詳細へ |
| `待機` | `MoteSettingsScreen.kt:281` | Interaction | 平常時の情報量ゼロ。状態行とボタンで代替（a11y注意） |
| `同期中`（ボタン） | `MoteSettingsScreen.kt:305` | Interaction | 状態行と同一行で同語二重。ボタンは disabled で表現 |
| `元に戻す` actionLabel | `MoteScreen.kt:417` | Redundant | カスタム Snackbar が上書き。未使用 |
| `quietSuccessMessages` 5件 | `MoteScreen.kt:734-738` | Redundant | 表示されないが抑止リスト。削除すると成功バナーが出るため、ViewModel側の設定も同時に削除する場合のみ |
| `Harmion に保存しました。` ほか ViewModel 成功通知 | `MoteViewModel.kt:393,595,643,660,859` | Redundant | 上記と同様、表示経路なし |
| `下書きを削除…` / `端末の記録を破棄…` | `LongCaptureScreen.kt:165,174` | Essential | 区別は保持。違いの説明をヘルプ/ダイアログへ |
| `追記の結果が未確定です。一致した場合だけ採用し、前回の操作は再送しません。` | `LongCaptureScreen.kt:314` | Essential | 1文目は状態、2文目は調停説明。2文目をヘルプへ分離可 |
| `送信結果を確認できていません` / `すでに保存されている可能性が…` | `SharedCaptureScreen.kt:208-210` | Essential | 重複リスクとデータ保全の警告。保持 |
| `（本文なし）` ×4 | `MoteScreen.kt` | Essential | 空表示と a11y に必要。保持 |
| `Inbox は空です` / `メモはありません` / `ピン留めはありません` | `MoteScreen.kt:453-455` | Essential | 空状態の説明として保持（サブエージェントの削除案は採らない） |
| `contentDescription = placeholder` | `MoteComposer.kt:108` | Essential | **維持（要修正）**。入力後も「メモを書く…」を読み上げる。label/`stateDescription` へ |

`削除しますか？`（`LongCaptureScreen.kt:261`）は対象が消える。文言削除ではなく、対象を確認ダイアログに残す設計修正が必要。

---

## 6. 管理Web UI

| 文字列 | 場所 | 分類 | 判断 |
|---|---|---|---|
| `REMOTE ACCESS` / `OWNER ACCESS` / `APPLICATION ACCESS` / `INTEGRATED CONNECTION` / `CONFLICT RESOLVER` / `CONTENT BROWSER` / `DATA PORTABILITY` / `INTERPRETER PROVIDERS` / `PEER CONNECTIONS` / `REMOTE REPLICA` / `LOCAL NODE` | `App.svelte:147,171`、各 `pages/*.svelte` | Redundant | 全削除。h1の英語言い換え |
| 8画面の導入段落 | `Apps:136-139`、`Conflicts:133-135`、`Connections:136-139`、`Contents:73-76`、`Export:32-34`、`Interpreters:87-90`、`Overview:63-64`、`Peers:174-176` | Redundant | 全削除。直下のUIで説明済み |
| `管理コンソール` | `App.svelte:192` | Redundant | 削除 |
| `WORKSPACE / {page}` | `App.svelte:210` | Redundant | 削除。ナビ選択とh1で自明 |
| フッター `Harmion / Worker replica (read-only)` / `Local administration` / `Cloudflare Access + 管理者トークン` / `この端末の所有者セッション` | `App.svelte:285-286` | Redundant | 全削除。ヘッダー状態と重複＋実装情報 |
| `harmion-admin run NODE_DIR` | `App.svelte:183` | Contextual help | 復旧手順としてヘルプへ |
| `リンクは2分、所有者セッションは30分有効です。` | `App.svelte:184` | Contextual help | ヘッダーの残時間で足りる。リンク期限のみヘルプへ |
| `承認待ちが {n} 件あります。` / `内容を確認して承認または拒否してください。` | `Connections.svelte:150-151` | Redundant | タブbadgeとカードで自明。削除 |
| `承認待ちの提供元が {n} 件あります。` / `提供元アプリが登録した内容を確認し…` | `Interpreters.svelte:100-101` | Redundant | 見出しbadgeとカードで自明。削除 |
| `承認待ちの要求` / `最近の履歴` / `最近の接続要求` | `Connections.svelte:199-201` | Redundant | 選択中タブと重複。削除 |
| `最新 32 件を表示します。表示中は5秒ごとに自動更新し…` | `Connections.svelte:202-205` | Redundant | 上限値とポーリング間隔の露出。削除 |
| `同期状態` の値 `同期情報を取得しました（詳細は Peer 画面で確認してください）。` | `Overview.svelte:184-185` | Redundant | 状態を何も伝えていない。削除 |
| `Content の内容` | `Contents.svelte:149` | Redundant | 同語反復。削除 |
| `左の一覧から本文・Kind・属性を確認できます。` | `Contents.svelte:181` | Redundant | 2ペインで自明。削除 |
| `読み取りました。内容の確認へ進んでください。` | `Peers.svelte:299` | Redundant | 入力欄への反映で自明。削除 |
| `接続要求はありません。アプリが接続を開始すると、ここに要求が表示されます。` の後半 | `Connections.svelte:210` | Redundant | 空状態の見出しで足りる |
| `Android のカメラで読み取るか、接続コードをコピーしてください。` | `Peers.svelte:368` | Redundant | L259 と重複。削除 |
| `管理モード` / `読み取り専用` / `Worker レプリカ` | `Overview.svelte:94` | Redundant | ヘッダーbadgeと重複。削除 |
| `Peer 登録` / `routing projection` / `Provisioning` / `デプロイ所有者の初期設定` | `Overview.svelte:97-106` | Redundant | 実装語。値のみに |
| 生enum `{overview.approval.mode}` | `Overview.svelte:178` | Redundant | 表示バグ。直後の日本語ラベルと二重 |
| `ブラウザセッションの窃取だけでは承認を完結させないための確認です。` | `Connections.svelte:158` | Redundant | 設計意図の露出。削除 |
| `この起動方法では R2 の承認を完了できません。` のフォールバック `R2 の承認を完了できません。` | `Connections.svelte:165` | Redundant | 直前の見出しと同義。削除 |
| `アプリは同名 Provider の置換を希望しています。…` | `Connections.svelte:271-273` | Essential | 失敗条件の説明。保持（確認ダイアログと重複する部分は整理可） |
| `承認と接続の違い` セクション全体 | `Interpreters.svelte:250-262` | Contextual help | 用語集は折りたたみ/ヘルプへ |
| `「接続中」は…WebSocket を開いている状態です。` | `Interpreters.svelte:167-170` | Contextual help | 実装語を含む常設説明。ヘルプへ |
| Providerヒント3種 | `format.ts:95-98` | Contextual help | 権限一覧で代替。ヘルプへ |
| `読み取り専用` badge（4箇所） | `App.svelte:212`、`Overview:118`、`Peers:193`、フッター | Essential/Redundant | ヘッダー/ページの状態は保持、フッターは削除 |
| `終了` | `App.svelte:242` | Essential | 何を終了するか不明瞭。削除ではなくラベル判断 |
| `errorMessage()` の raw 例外 | `api.svelte.ts` | Essential | **維持（要修正）**。英語例外をマッピング |
| `確認して実行`（既定ラベル） | `Confirm.svelte:6` | Essential | 汎用すぎる。個別ラベル化は要判断 |

注: `APPLICATION ACCESS` などのeyebrowは「装飾」、導入段落は「設計意図の説明」で、AGENTS.md の2項目に直接該当する。管理UIは開発者向けという前提でも、h1の言い換えを常設する必要はない。

---

## 7. native CLI / daemon / admin

CLIは `--help` がヘルプ面なので、Contextual help 86件の大半は「維持」である。削除・移動の対象は以下に限られる。

| 文字列 | 場所 | 分類 | 判断 |
|---|---|---|---|
| `Request files require an owner-only parent directory and must not exist.` | `apps/cli/src/cli.rs:7` | Contextual help | ルートの `after_help` から対象8コマンドの引数helpへ移動 |
| `State directory. Defaults to ~/.local/share/harmion/admin-ui.` ×3 | `apps/admin/src/main.rs:32,38,46` | Contextual help | `--ui-dir` をglobalに統合し1箇所へ |
| `--approval` help後半と `session` variant説明の重複 | `apps/admin/src/main.rs:49-50`、`approval.rs:26-28` | Contextual help | 片方に統合 |
| `{command}: ノード/証明 {hex}` | `apps/cli/src/main.rs:246` | Redundant | command名とhexの補間を削除。動作名だけ残す |
| `{command}: {path} の内容を適用` | `apps/cli/src/main.rs:260` | Redundant | 同上。`trust-recovery-peer` では誤説明 |
| `\n{}`（change-grants のgrants生JSON） | `apps/admin/src/api.rs:482` | Redundant | 端末で読めないJSONダンプ。app_id/revokedのみに |
| `installed $tag` | `scripts/harmion-update:238` | Redundant | 直後の `harmiond --version` で足りる |
| `-h, --help  Show this help` | `scripts/harmion-update:38` | Redundant | 自明 |
| `daemon restart may be required; use --restart or restart harmion@$instance.service` | `scripts/harmion-update:240` | Interaction | `--restart` 使用時/未起動時は出さない |
| `Reset the signed-HTTPS replay clock after a wall-clock reversal.`（前半） | `apps/daemon/src/cli.rs:21` | Contextual help | 実装理由。`The daemon must be stopped…` は残す |

**要修正（削除ではない）**:
- `harmion: InvalidHex` / `harmion: Http(404)` は `crates/client/src/lib.rs:55`、`crates/protocol/src/record.rs:12` の `{self:?}`。ユーザー語へのマッピングが必要。
- daemon/admin の失敗が `ERROR … error=No such file or directory (os error 2)` の1行のみ。対象パスと対処を出す必要。
- 位置引数のhelp欠落（`<TOKEN>`、`<SOURCE>`、`<KINDS>`、`<VERSIONS>`、daemon `run` の7オプション）。これは冗長の逆で、不足。
- `sync` の `<ID>` はContent IDではなくピアNodeId。value_nameの変更は要判断。

---

## 8. Wiki GUI

| 文字列 | 場所 | 分類 | 判断 |
|---|---|---|---|
| `Wiki へようこそ` / `ノート` / `ページはまだありません。最初の1枚を作りましょう。` | `main.rs:2760-2769` | Redundant | 空状態の装飾スタック。直下の `最初のページを作成` が行動を担う |
| `ライブラリ` | `main.rs:2178` | Redundant | 直下2行で自明。削除 |
| `タグ付きページはまだありません。` | `main.rs:2210` | Redundant | チップ欠如で自明。削除 |
| `ページ` / `最近のページ`（サイドバー見出し） | `main.rs:2266-2270` | Redundant | ナビ選択と重複。削除 |
| `{n} ページ · {n} スニペット` | `main.rs:2318` | Redundant | 各見出しに既出。削除 |
| `ページがありません` / `タブがありません。` | `main.rs:2601,2711` | Redundant | 二重の空状態。片方に |
| `開く`（コンテキストメニュー2箇所） | `main.rs:2383,2452` | Redundant | 行クリックと同一動作。削除 |
| `ページ・本文・タグを検索` | `toolbar.rs:119` | Redundant | 虫眼鏡＋`Ctrl+K` で自明。範囲説明はヘルプへ |
| `プレースホルダ` | `main.rs:2816` | Redundant | 実装語。削除 |
| `閲覧ビュー · 保存は Neovim` | `main.rs:4704` | Redundant | 設計意図の露出。削除 |
| `競合情報はまだありません。` | `main.rs:4715` | Redundant | 実装概念。削除 |
| `接続すると使えます。` | `main.rs:4651` | Redundant | 未登録表示と重複。削除 |
| `Ctrl+N で新規ページ、Ctrl+K で検索できます。` / `Ctrl+K で検索できます。接続後に Ctrl+N で新規ページ。` | `main.rs:2783,2785` | Contextual help | `?` オーバーレイと完全重複。削除 |
| `Ctrl+E で現在のページを Neovide で開きます。` | `main.rs:4669` | Contextual help | toolbar/statusbar の kbd と重複。警告時のみ |
| `コピー`（コードブロック） | `main.rs:3446` | Redundant | アイコンのみで自明（初見の気付きやすさは要確認） |
| `共通タグ: {kind_label}` | `main.rs:4517` | Redundant | タグではなくkindを出しており誤情報。削除 |
| `2-hop` chip / `2-hop リンク` / `次に読む` / `2-hop をすべて見る` | `main.rs:4337,4375,4488`、`shelf.rs:58` | Redundant | 同一概念の4表記。1つに統合（要判断） |
| `クリックで閉じる ·` / `クリックで表示 ·` | `shelf.rs:79,81` | Interaction | シェブロンと開閉状態で代替 |
| `ページを開くとリンクが表示されます。` / `ページを開くと情報が表示されます。` | `main.rs:4292,4580` | Redundant | ページ未選択は自明。削除 |
| `2-hop のリンクはまだありません。` / `関連スニペットはありません。` / `発リンクはありません。` / `バックリンクはありません。` | `main.rs:4348,4407,4437,4452` | Redundant | 見出しの件数0と同じ。削除 |
| `入力中は一部のショートカットを無効にします。` | `main.rs:5048` | Redundant | 曖昧な実装都合。削除 |
| `Neovide · 起動済み/接続済み/未起動` / `Neovide · {title}` | `neovide.rs:93,334-339` | Redundant | 未描画の死文。状態バーと三重。削除 |
| `nvim --server failed…` / `niri msg…` / `nvim --server exited…` / `niri … invalid JSON` | `neovide.rs:75-81,114-120,151-154,496-508` | Redundant | 生の外部コマンドエラー。ログへ |
| `neovide / alacritty / foot / kitty / xterm のいずれも見つかりません` | `neovide.rs:327` | Redundant | 端末名列挙は実装詳細。削除 |
| `Wiki`（パンくずとサイドバー見出しの二重） | `toolbar.rs:17-19`、`main.rs:2314` | Redundant | 1つに |
| `本文に目次はありません。` | `main.rs:4035` | Redundant | 空は見て分かる。削除 |

注: Wiki GUI は `frozen v5 shell` と明記されており、コピー変更がデザイン凍結に触れる可能性がある。実行前に `docs/wiki-ui-design.html` との差分を確認する必要がある。`↑ ↓ 選択` / `Enter 開く` は非自明操作の説明なので保持。

---

## 9. 実装優先順位（提案）

### P0: 判断不要・低リスク

1. タブ字幕4件、管理UI eyebrow 11件、`管理コンソール`、`WORKSPACE`、管理UIフッター、Wikiの空状態装飾・サイドバー見出し。
2. 重複見出し・件数: Apps の字幕とピル、Peers の件数と同期エラー見出し、Peers の `状態` 行、Home の `現在`/`ノード`、管理UIのタブ見出し・callout。
3. 二重フィードバック: Settings のインライン `コピーしました`、Peers の `更新`、Mote の `待機`/`同期中` の片方。
4. 死文: `strings.xml` 3件、Wiki の未描画文字列。Mote の `quietSuccessMessages` は抑止リストなので単独削除しない。

### P1: 小さなUI変更を伴う

5. Home のヒーローと同期カードの統合（状態・最終成功・停止理由を1箇所へ）。
6. 参加承認待ちの3箇所（Onboarding / Home / PendingJoin）の統合と、`非表示` 後の復帰導線。
7. Mote の接続状態4系統を共通コンポーネント化し、字幕の常設をやめる。
8. 管理UIの承認モード・権限説明をヘルプ/ツールチップへ寄せる。
9. 進捗ラベル削除に合わせた `stateDescription` / `liveRegion` の整備。

### P2: プロダクト判断が必要

10. 生エラーのマッピング方針（Harmion `lastError`、通知、管理UI、Wiki、CLI）。
11. 用語・言語の統一（`Peer`/`ピア`、`Content`/`コンテンツ`、`参加コード` ほか）。
12. About のビルド詳細を「サポート情報」として残すか、ヘルプへ移すか。
13. Mote の `Inbox`/`All` をトグルに再設計するか。
14. Wiki のデザイン凍結範囲と、`2-hop` 用語の扱い。
15. CLI の `--help` 言語（英語/日本語）と、位置引数helpの追加。

---

## 10. 未決事項

1. **ヘルプ面が存在しない**。本レポートの「ヘルプへ移動」は、Android/Mote/管理UI/Wiki のどこに置くかを決めないと実行できない。ツールチップ、情報ダイアログ、`?` オーバーレイ、README のどれかをサーフェスごとに決める必要がある。
2. **言語ポリシー**。Android/Mote は日本語UI、管理UI/CLI は英語混在。管理UI/CLI を開発者専用と割り切るなら、`R2`/`TTY`/`grant` は維持可。一般所有者も使うなら用語集が必要。
3. **生エラーの扱い**。`errorLabel` の raw fallback を消すとデバッグ情報が落ちる。ユーザー向け文言＋ログ/コピー可能な詳細、の二層化が必要。
4. **アクセシビリティの代替**。進捗ラベル、`（本文なし）`、空状態、タブアイコンを削る場合は、TalkBack の読み上げを別途確認する。実行時の画面確認は未実施。
5. **同時編集**。監査中に `2edb62f` / `92ff053` がコミットされ、行番号が一部ずれた。本レポートは `92ff053` 基準。実装前に最新HEADで再確認すること。
6. **Mote の未コミット変更**（スワイプ方向）はコピーに影響しないが、作業ツリーは流動的。
7. **Wiki GUI の位置づけ**。配布物か実験的UIかで、`発リンク`・`2-hop`・デザイン凍結の扱いが変わる。
8. **管理UIの `終了`**。セッション終了かアプリ終了か。文言変更は本監査の範囲外だが、誤操作リスクとして記録。

---

## 11. 参考: 主要な維持対象（削除しない）

- 破壊的確認: 退役、削除、失効、リカバリコード再表示不可、自己退役の再参加不可。
- セキュリティ: 要求権限、強い権限、未検証データ、パッケージ/署名、所有者承認、端末の `y` 承認。
- エラー/復旧: `再試行`、`再接続`、`承認をリクエスト`、`照合して採用`、`閉じる`。
- クロスデバイス手順: QRを既存ノードで読む、参加要求の承認、共有リンクの保存。
- データ保全: Mote の「本文は端末に残る」保証、未確定時の再送しない説明、下書きの保持。
- アクセシビリティ: `戻る`、`閉じる`、`ライト`、`ピアの詳細を表示`、QR alt、`liveRegion`。
- 空状態: Mote の `Inbox は空です` など（視覚コピーを消す場合はセマンティクスで補う）。
- 通知: 競合件数、保存異常、常駐状態、ウィジェットの `開く`/`同期`。

---

## 12. 実装状況（2026-11-27）

アプリコードを変更し、本コミットに含めた。対象は Harmion Android、Mote、管理Web UI。CLI と Wiki GUI は未着手。

### 12.0 用語方針（改訂）

初回監査では日本語へ寄せる提案を含めていたが、ユーザー判断により**英語の製品語を優先**する方針に変更した。

- `Inbox` / `All` は維持。
- 短いメモは `short capture`、長いメモは `long capture`。
- `ピア` ではなく `peer`。
- 状態値は `active` / `retired` / `pending` / `conflict` / `unconfirmed`。
- 管理UIは `Peer` / `Content` / `Export` / `Node ID` / `Cluster ID` を維持。
- 日本語は操作・説明・エラーに残す。

### 12.1 Harmion Android

削除:
- タブ字幕4件と、タブアイコンの重複 contentDescription。
- Home の `アプリを開くと再開します`、ヒーローの `最終 …` と停止理由、`自動同期はオフです` カード、`参加の承認待ち` カード、`現在` 行、`ノード` 見出し。
- Peers の `更新` ボタン、同期エラー見出し、件数サブタイトル2件、`状態` 行、メンバー空状態の説明、共有シートのタイトル。
- Apps の件数サブタイトル、`承認待ち`/`承認済み` ピル、空状態の説明、`所有者の生体認証で確定します`。
- Approval の確認サブタイトル、期限サブタイトル、操作バーの説明2種。
- Conflicts の `競合` シート見出しと `競合を解決` 見出し。
- About の `Androidノード`、`リンク` 見出し。
- スキャナの `HARMION`、未使用 strings.xml 3件。
- Settings のバージョン行、復元説明、リカバリコードのToast（インライン表示は維持）。

用語統一:
- `ピア` → `peer`。端末数は `台`、ウィジェット/常駐通知は `peer N 台`。
- 状態値は `active` / `retired` / `pending` / `conflict` / `unconfirmed`。
- 招待（既存→新規）は `招待コード`/`招待QR`、参加要求（新規→既存）は `参加要求コード`/`参加QR`、状態は `参加の承認待ち`。
- `指定した種類`、`membership conflict`。

### 12.2 Mote

削除:
- 長文下書きカードの `再開`。
- Settings の `操作の反応`。
- 下書き削除確認を `下書きを削除しますか？` に対象明示。

維持（初回削除から戻したもの）:
- ヘッダーの `Mote`。
- About の `種別`/`ビルド日時`/`パッケージ`/`リビジョン` と `Harmion のメモ`。
- `Inbox` / `All`。

用語統一:
- 短いメモは `short capture`、長いメモは `long capture`。汎用は `capture`。
- フィルタは `Inbox` / `All`、復元操作は `Inbox に戻す`。
- ピン操作は `ピン留め`/`ピン留め解除`。
- `短文`/`Capture` の可視文字列を `short capture`/`capture` に統一（`Kind` は `種類`）。

維持:
- `quietSuccessMessages` は抑止リストなので削除していない（削除すると成功バナーが出る）。
- 空状態、オフライン保持の保証、未確定結果の警告は維持。

### 12.3 管理Web UI

削除:
- 英語eyebrow 11件、ページ導入段落8件、`管理コンソール`、`WORKSPACE / …`、フッター。
- Connections の承認待ちcallout、ポーリング説明、タブ重複見出し、設計意図の一文、重複フォールバック。
- Interpreters の承認待ちcallout、空状態の `provide` 説明。
- Overview の `同期情報を取得しました…`、生enum、`routing projection`、重複する読み取り専用バッジ。
- Contents の `Content の内容`、2ペイン説明。
- Peers の読み取り完了メッセージ、QRダイアログの重複説明、読み取り専用バッジ。
- Merge の操作説明。

用語:
- `Peer` / `Content` / `Content ID` / `Export` / `Node ID` / `Cluster ID` を維持（日本語化を撤回）。
- `終了` → `セッションを終了`。
- ヘッダーから左側のWORKSPACEを消したため、アクションを右寄せに調整。

維持:
- `Interpreter`、`Kind`、`Provider`、`Grant JSON`、`R2`、`TTY` は技術用語として維持。
- セキュリティ警告、破壊的確認、空状態、アクセシビリティラベルは維持。

### 12.4 検証

- `gradle --offline :app:compileDebugKotlin :mote:compileDebugKotlin :app:processDebugResources -x buildRust` → BUILD SUCCESSFUL。
- `svelte-check` → 0 errors / 0 warnings。
- `prettier --check` → all matched files。
- `vite build` → success。

### 12.5 残り

- CLI: `after_help` の移動、`harmion-update` の成功行/help行、`--ui-dir` の重複、生enumエラー（`{self:?}`）。
- Wiki GUI: 空状態装飾、`プレースホルダ`、`閲覧ビュー · 保存は Neovim`、`nvim`/`niri` の生エラー、`2-hop` 用語。デザイン凍結の確認が必要。
- 横断: 生エラーのマッピング、ヘルプ面の設計、言語ポリシー、実行時のTalkBack確認。
