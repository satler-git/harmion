# ロードマップ

状態: 計画 / 対象: リポジトリ全体 / 最終更新: 2026-11-28
正となる実装: 各項目の「根拠」列

この文書は「何を作るか」ではなく「何を先に決め、何を先に使い始めるか」を持つ。順序は確定ではない。
番号（`D-` 決定、`S-` スパイク、`W-`/`M-`/`L-`/`C-`/`A-`/`P-`/`T-` 作業）で他文書から参照する。

**Phase という語は使わない。** 現行文書には Mote 製品 Phase 1–5、Mote HUD P0–P5、Android ノード Phase 0–5 の3体系があり、
「Phase 3 を保留」がどれを指すか一意でない。以後はこの文書の ID で参照する。

## 1. 決定済み（2026-11-28）

| ID | 論点 | 決定 | 含意 |
|---|---|---|---|
| D-1 | Mote の音声認識 | 自前の ASR を書かない。短文の音声は OS の音声入力（IME）に頼る | `VoiceProbeService` は実験用のまま。認識エンジン抽象化を Mote に作らない |
| D-2 | Mote の録音 | 背景継続・60分連続・元音声の保持は保留（要件からは消さない） | OS 入力は Activity フォーカス必須で、この3点を代替できない。将来の再開候補として残す |
| D-3 | Pebble の音声 | 時計側の dictation を使う。認識は電話側のクラウド（WisprFlow）で行い、Mote は文字を受け取るだけ | Mote に音声入力が無くても Pebble 経路は成立する（§2.1） |
| D-4 | Pebble の経路 | watch → AppMessage → Mote(Android) → 既存の保存経路 | App API が loopback 限定なので watch からノードへ直結しない |
| D-5 | Mote → Wiki の直結 | やらない。整理されていない Mote のデータは Wiki に入れない | 将来 `capture-copy` kind を追加し、整理済みのコピーだけを Wiki へ移す |
| D-6 | Mote データの編集主体 | Wiki 側から Mote のデータを編集しない | 一方向（Mote → Wiki）を保つ。双方向同期を作らない |
| D-7 | Wiki のデイリーノート | 作る。短文ストリームから選別して1本に統合し、Wiki へ入れる | [wiki-app-requirements.md](apps/wiki/requirements.md#L91) では Could 扱い。v1 の範囲を広げる決定 |
| D-8 | Ledger の計算 | 自宅 PC の Interpreter に置かない。hledger ライブラリをビルドして Android アプリにリンクする | [node の要件](apps/node/requirements.md#L555-L565) の「方式未決定」を決めた。計算は Interpreter の有無に依存せず、アプリが常に持つ。最初に通すのは GHC のクロスビルド（S-2） |
| D-9 | Ledger の範囲 | journal の記入・閲覧に加え、グラフ/統計、レシート撮影 OCR、領収書メール取込、予算管理 | 台帳アプリではなく「記録＋取込＋集計」のアプリになる。取込は Agent 基盤（A-1）を共有 |
| D-10 | takusu の接続 | 今はやらない。設計を先に詰める | 理由は原子性（§2.3）。`takusu-contracts::Storage` への差し替えは設計後 |
| D-11 | 出荷/CI | 早期に固める | 実体はほぼ完成している（§2.4）。残りは小さく、アプリ追加の前提 |
| D-12 | 既存ノート | Scrapbox ×2、Obsidian ×2（どれも小規模）。エクスポート後に Agent で整理を回す可能性がある | 取込は「変換」と「整理」の2段に分かれる（W-1 / W-2） |

## 2. 現状（事実）

- native / Android ノード / Mote は実使用に入っている。Wiki は定着していない。
- Wiki の実装は資料より進んでいる。検索（FTS5 trigram + NFKC）、OAuth 統合接続、Neovim 連携は動作する。
  node モードの `save` / `create_page` / `create_snippet` / `convert_fence` / `delete` / `update_meta` は使える
  （[store.rs:900](../apps/wiki-server/src/store.rs#L900)）。
- 文書の古い記述: [apps/wiki/README.md](../apps/wiki/README.md) の「create/convert/delete は 501」、
  [shipping.md](ops/shipping.md#L3) の「CI なし／Nix package なし」は、いずれも現状と合わない。

### 2.1 Pebble の音声は電話側で処理される

SDK 4.33.1 の `emery`（Pebble Time 2）に `dictation_session_create/start/stop` がある
（`~/.local/share/pebble-sdk/SDKs/4.33.1/sdk-core/pebble/emery/include/pebble.h:2118-2200`）。

- 認識は**時計では行わない**。`dictation_session_create` は「電話アプリが未接続、または音声 dictation 非対応なら NULL」を返す。
- 失敗理由に `FailureConnectivityError`（BT もインターネットも無い）、`FailureRecognizerError`（クラウド認識失敗）、
  `FailureDisabled`（利用者設定で無効）がある。
- 成功時のコールバックに**転写済みの文字列**が渡る。時計側の UI で確認・採用する。

したがって Pebble 経路は Mote の ASR に依存しない。逆に言えば **OS 音声入力に寄せる決定（D-1）と Pebble の決定（D-3）は同じ決定**で、
どちらも電話の認識器の精度に乗る。未確認なのは「Android の現行 Pebble アプリで dictation が有効か」だけ（S-1）。

クラウド認識器は WisprFlow。電話の OS 入力（IME）とは別の認識器なので、精度と遅延を一律に扱わない。M-1 では電話の IME と時計の dictation を別々に測る。

### 2.2 時計は「画面を出さずに話す」を担当する

OS キーボードの音声入力は Activity のフォーカスが要る。画面消灯中の連続録音はできない（D-2）。
一方 PT2 の dictation は「時計を上げて1発話」を画面ロック中でも成立させる。**保留した要件の一部は時計が引き受ける**。

### 2.3 複数 Content の原子性（takusu の懸念は仕様上の制約）

[specification.md](core/requirements.md#L53) が「初期版では複数 Content をまたぐ原子的な更新を保証しない」と明記している。
D-10 の判断は正しい。影響範囲は限られる。

- 影響する: 1操作で複数 Content を整合させたいもの（takusu のエンティティ群、レシート1枚から journal＋予算＋勘定科目を作る処理）。
- 影響しない: 1 Content だけを書くもの（デイリーノートの生成、Mote の短文）。取込のように「N 件を独立に作る」ものは
  部分成功をレポートすればよい。

解くなら冪等な `request_id` と補償（[app-api.md](core/app-api.md) の保存 receipt が土台）で、仕様変更として扱う（C-4）。

### 2.4 出荷はほぼ完成している

- タグ起動の署名リリースが動く（[release-android.yml](../.github/workflows/release-android.yml)）。
  APK・mapping・証明書 SHA-256・SHA256SUMS を Release へ出す。
- アプリ追加は [release-apps.toml](../android/release-apps.toml) に `[[app]]` を1つ足す形になっている。
- `versionCode` の単調増加は [release-app.sh:89](../android/scripts/release-app.sh#L89) が既に検査する。

残りは「トリガーのタグ列挙が `node-v*`/`mote-v*` の直書き」「native の Nix package が古い文書のまま」程度（P-1）。

## 3. ゲート

機能の完成ではなく、実使用で区切る。

| ゲート | 意味 | 現状 |
|---|---|---|
| G1 | 毎日 Mote で記録し、週1回 Wiki で整理している | Mote は達成、Wiki が未達 |
| G2 | スマホから Wiki に書ける（時計・デイリーノート経由でよい） | 未着手 |
| G3 | 外部エージェントが grant 経由でデータを読める | 未着手 |
| G4 | 2つ目のアプリが同じ基盤で動く（Ledger） | 方式決定済み、実装前 |

## 4. 作業項目

サイズ: S ≤3日 / M 1–2週 / L 3週以上。

### 4.1 Wiki（G1 の本体）

| ID | 項目 | サイズ | 前提 | 完了条件・根拠 |
|---|---|---|---|---|
| W-1 | 取込: Scrapbox / Obsidian のエクスポートを Wiki 形式へ変換し、node へ流し込む | M | S-3 | `import` は node モードで 501（[store.rs:825](../apps/wiki-server/src/store.rs#L825) の許可リストに無い）。まずここを直す。形式は [import.rs](../apps/wiki-server/src/import.rs) の frontmatter 形式に合わせる |
| W-2 | 取込後の Agent 整理（リンク修復・重複統合・タグ付与・見出し再構成） | M | A-1 | 取込結果を人が読める状態にする。D-12 の「整理」側 |
| W-3 | デイリーノート | M | D-7, U-1 | 短文ストリームから選別 → 1本のページを生成 → Wiki へ。単一 Content の書込みなので原子性の制約を受けない |
| W-4 | 鮮度: 起動時スナップショットをやめ、ノードの Subscribe に繋ぐ | S–M | — | [store.rs:120](../apps/wiki-server/src/store.rs#L120) の一括ロードを置き換える。ノード側の Subscribe/Resume は実装済み（[app-api.md](core/app-api.md#L113-L120)） |
| W-5 | 添付画像・SVG の埋め込み表示 | S | — | GUI の既知ギャップ |
| W-6 | 競合: 3-way マージ、アプリからの kind/delete 競合解決 | M | — | 複数端末で書く前に必要。それまでは後回し |

### 4.2 Mote（G1・G2 の入力側）

| ID | 項目 | サイズ | 前提 | 完了条件・根拠 |
|---|---|---|---|---|
| M-1 | OS 音声入力で日常運用を1週間試し、不足を記録する | S | D-1 | 不足が要件（背景・長時間・元音声）に戻るなら D-2 を見直す |
| M-2 | 整形（LLM）と派生 Content | M | A-1, U-4 | 旧 Phase 4。`derived_from` の定義が前提（[mote-design.md](apps/mote/design.md) で未定義） |
| M-3 | 検索 | S | — | 要件のみ（[mote-search-requirements.md](apps/mote/search.md)）、コード無し |
| M-4 | デイリーノートへ追記する経路の実装 | M | U-1 | W-3 の入力側。Mote の Inbox とは別のストリームにする（D-7） |
| M-5 | Pebble 連携: AppMessage 受信サービス、UUID 登録、短文 capture 生成、outbox 再利用 | M | S-1 | 切断・再送・認可エラー表示まで。watch に認可情報を置かない |

### 4.3 Ledger（G4）

| ID | 項目 | サイズ | 前提 | 完了条件・根拠 |
|---|---|---|---|---|
| L-1 | **hledger-lib を aarch64-linux-android へビルドし、JNI から呼ぶ** | L | S-2 | FFI 境界は「journal の文字列 → レポート JSON」に限定し、hledger の型を境界へ出さない |
| L-2 | Android アプリ本体（Compose）: journal の記入・閲覧・残高・register | L | L-1 | データは Content（kind=ledger）の本文。端末の DB を正本にしない |
| L-3 | グラフ・統計・予算 | M | L-1 | hledger の report を使い、描画だけアプリ側 |
| L-4 | レシート撮影 → OCR → 候補 → 承認 → journal 追記 | M | A-2 | D-9。単一 Content への追記で原子性の制約を受けない |
| L-5 | 領収書メールの自動取込 | M | A-2 | 取得経路（IMAP / Gmail API）と権限の設計が別途必要 |
| L-6 | Wiki の ledger 表現との整合 | S | — | Wiki は `!` フェンス → snippet → `ledger/balance` の設計を持つ（[Wiki の要件](apps/wiki/requirements.md#L275)）。**同じ Content 形式**にすること。計算はアプリが持ち、Interpreter の有無に依存させない |

### 4.4 認可・admin

| ID | 項目 | サイズ | 前提 | 完了条件・根拠 |
|---|---|---|---|---|
| C-1 | 管理画面の5画面化と承認受信箱（作業中・未コミット） | M | U-5 | [harmion-admin-design.md](ops/admin.md)。**パスキーを前提にしない形で先に完成させる** |
| C-2 | 監査の表示とエクスポート | S | — | R2 の監査ログは実装済み、UI が無い（[owner-authentication-design.md](core/owner-auth.md#L359-L367)） |
| C-3 | `admin.secret` の二段階ローテーション | S | — | 期限内の資格情報が恒久の所有者権限になる問題 |
| C-4 | 複数 Content の原子性・補償の設計 | M | — | §2.3。takusu（T-1）とレシート取込（L-4）の前提 |
| C-5 | パスキー（WebAuthn） | M | S-4 | `https://localhost` 移行が前提（RP ID に IP を使えない） |
| C-6 | ノードの表示名 | S | — | Node ID の代わりに出す名前をノードごとに持つ。管理 UI・同期状態・membership 一覧で使う。まずローカル保存で始め、他ノードへ同期するかは決めない。無くても動くが、ノードが3台を超えると識別が効く |

### 4.5 Agent・LLM 基盤（G3）

| ID | 項目 | サイズ | 前提 | 完了条件・根拠 |
|---|---|---|---|---|
| A-1 | LLM バックエンドの抽象化 | M | — | Mote の整形（M-2）、取込の整理（W-2）、Ledger の取込（L-4/L-5）が共有する。**アプリごとに作らない** |
| A-2 | 非構造 → 構造の取込パイプライン（レシート・メール・スクショ） | M | A-1 | 抽出 → 候補 → 承認 → 保存。承認 UI は C-1 の受信箱と共通化 |
| A-3 | MCP（読み取り） | M | C-1 | 通常の App API クライアントとして grant を受ける（[architecture.md](core/architecture.md#L195)）。管理権限を与えない |
| A-4 | MCP（書き込み・承認付き） | M | A-3, C-2 | 書込みは R2 相当の承認を通す |

### 4.6 その他

| ID | 項目 | サイズ | 前提 | 完了条件・根拠 |
|---|---|---|---|---|
| P-1 | 出荷: トリガーを `release-apps.toml` から導出、native package の文書更新 | S | — | アプリを増やす前の整備（§2.4） |
| P-2 | ドキュメント再編（索引・ディレクトリ・状態ヘッダ） | S | — | [README.md](../README.md#L79) の索引は 39本中13本が漏れている |
| T-1 | takusu の接続設計 | L | C-4 | D-10。`Storage` trait への差し替えは原子性の設計後（takusu リポジトリの `crates/takusu-contracts`） |

## 5. スパイク（順序より先に潰す）

| ID | 問い | 判定にかかる時間 | 外れた場合 |
|---|---|---|---|
| S-1 | Android の現行 Pebble アプリで dictation が有効か（`FailureDisabled` にならないか） | 実機30分 | M-5 を保留し、時計は表示専用へ |
| S-2 | hledger-lib が aarch64-linux-android でビルド・リンクできるか | 半日 | 落ちた場合だけ D-8 を再検討する（Wasm 実行環境か独自実装）。通る前提で L-1 を書いてよい |
| S-3 | Scrapbox / Obsidian の実エクスポートを見て、変換の対応表を作る | 1時間 | W-1 の形式決定 |
| S-4 | 認可: SSH assertion で完成させるか、`https://localhost` へ移してパスキーを採るか | 半日 | C-1 の完成形が変わる |
| S-5 | Haas-ja の量子化モデルがスマホで実用的な速度・精度で動くか | 1–2日 | 保留のまま（D-1 は影響を受けない） |

## 6. 推奨順序（提案）

| Stage | 内容 | 期間感 |
|---|---|---|
| 1 | P-2（文書再編）、P-1（出荷の整備）、C-6（ノードの表示名）、S-1・S-2・S-3・S-4 | 1週間 |
| 2 | W-1 → W-3 → W-4（Wiki の定着＝G1）、M-1（OS 入力の実測） | 2–3週間 |
| 3 | L-1 → L-2（S-2 が通った場合）、C-1 → C-2、A-1 | 3–4週間 |
| 4 | W-2、M-2、A-2、L-4 | 3–4週間 |
| 5 | A-3、M-5（S-1 が通った場合）、C-3・C-4 | 3–4週間 |
| 6 | T-1、L-5・L-6、W-6 | 以降 |

順序が変わる条件:

- S-2 が落ちた場合、L-1 が Stage 3 から外れ、代わりに M-2 と A-1 が前倒しになる。
- S-1 が落ちた場合、M-5 が消え、Stage 5 が縮む。
- G1 に2週間で届かない場合、Wiki の残りを止めて不足を要件に戻す（機能追加を先行しない）。

## 7. 未決

| ID | 論点 |
|---|---|
| U-1 | Mote から Wiki のデイリーノートへ追記する権限の設計。書込み grant は kind 単位で、特定の1ページだけに絞る表現がない。案は3つ: ① デイリーノート専用の kind を作り、Write をその kind に限定する ② Wiki が `daily/append` を Interpreter として提供し、Mote は grant を持たずに call する ③ Mote 側に置いて人がまとめる |
| U-2 | 取込時の Agent 整理の粒度（全文整形 / リンク修復 / 重複統合 / タグ付与のどれまで） |
| U-3 | `capture-copy` kind の形（D-5 の将来分）。誰が・いつ整理済みと判定するか |
| U-4 | Mote の整形結果を Content として持つか、本文の書き換えにするか（`derived_from` の設計） |
| U-5 | 進め方: C-1 を SSH assertion のまま完成させるか、S-4 の結論を待つか |
