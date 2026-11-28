# ドキュメント

最終更新: 2026-11-28

ここが唯一の索引。`docs/README.md` に無い文書は存在しない扱いにする。ルートの [README.md](../README.md#L79) は入口だけを持つ。

## 読み方

- 各文書の先頭に `状態` / `対象` / `最終更新` / `正となる実装` を置く。まず `状態` を見る。
- `状態` は **要件 / 設計 / 実装済み / 手順 / 計画 / 記録** の6語だけを使う。
- 作業の番号は [roadmap.md](roadmap.md) の ID（`D-` 決定、`S-` スパイク、`W-`/`M-`/`L-`/`C-`/`A-`/`P-`/`T-` 作業）で参照する。
- **`Phase` という語は使わない。** 旧文書の Phase 番号は互いに衝突しており、どれを指すか一意でない。

## core — 基盤

| 文書 | 内容 | 状態 |
|---|---|---|
| [requirements.md](core/requirements.md) | 仕様と設計方針（正本） | 要件 |
| [architecture.md](core/architecture.md) | パッケージ構成と責務 | 設計 |
| [domain.md](core/domain.md) | 内部表現・同期・競合の初期設計 | 設計 |
| [app-api.md](core/app-api.md) | 取得・編集・購読・OAuth の契約 | 実装済み |
| [membership.md](core/membership.md) | 参加・退役の設計判断 | 実装済み |
| [owner-auth.md](core/owner-auth.md) | 所有者認証と R2 承認 | 実装済み（パスキー以降は設計） |
| [owner-auth-memo.md](core/owner-auth-memo.md) | 所有者鍵・OS 鍵保管の将来案 | 記録（未実装の検討） |
| [search.md](core/search.md) | 標準検索の草案 | 計画 |
| [notes.md](core/notes.md) | 実装寄りの詳細（契約ではない） | 記録 |
| [cloudflare.md](core/cloudflare.md) | 中継（Workers/DO）の設計 | 設計（workerd まで実装） |

## apps — 利用アプリ

| 文書 | 内容 | 状態 |
|---|---|---|
| [node/requirements.md](apps/node/requirements.md) | Android ノードの要件と実装指針 | 要件 |
| [node/design.md](apps/node/design.md) | Android ノードの設計と実装状態 | 実装済み |
| [mote/requirements.md](apps/mote/requirements.md) | Mote の要件 v0.2 | 要件 |
| [mote/design.md](apps/mote/design.md) | Mote の設計と起動手順 | 実装済み |
| [mote/voice.md](apps/mote/voice.md) | 音声入力の検証（モデル比較・録音） | 計画（保留。D-2） |
| [mote/recovery.md](apps/mote/recovery.md) | 失敗時の振る舞い | 設計 |
| [mote/cli.md](apps/mote/cli.md) | Mote CLI | 実装済み（V1） |
| [mote/search.md](apps/mote/search.md) | Android のライブ検索 | 計画 |
| [mote/desktop.md](apps/mote/desktop.md) | デスクトップ版 UX の草案 | 計画 |
| [mote/hud.md](apps/mote/hud.md) | デスクトップ HUD の実装計画 | 実装済み |
| [mote/hud-install.md](apps/mote/hud-install.md) | HUD の導入 | 手順 |
| [wiki/requirements.md](apps/wiki/requirements.md) | Wiki の要件（v1 草案） | 要件 |
| [wiki/design.md](apps/wiki/design.md) | GUI の刷新とフレームワーク判断 | 実装済み |
| [wiki/status.md](apps/wiki/status.md) | Wiki の実装状況 | 実装状況 |

Ledger は [roadmap.md](roadmap.md) の `L-` 項目として未着手。文書は実装開始時に作る。

## ops — 手順と出荷

| 文書 | 内容 | 状態 |
|---|---|---|
| [shipping.md](ops/shipping.md) | 出荷（CI/CD・署名・secret）の設計 | 設計 |
| [ci-plan.md](ops/ci-plan.md) | CI/CD の実行プラン | 実装済み（Android と Worker） |
| [admin.md](ops/admin.md) | harmion-admin の再設計 | 実装済み（パスキーは設計） |
| [admin-worker.md](ops/admin-worker.md) | Worker 管理画面の情報設計 | 実装済み（P0 + P1） |
| [ui-copy.md](ops/ui-copy.md) | UI コピーの監査と方針 | 記録（P0 実装済み） |
| [linux-service.md](ops/linux-service.md) | Linux user service と日常運用 | 手順 |
| [native-quickstart.md](ops/native-quickstart.md) | native 複数ノード同期 | 手順 |
| [android-release.md](ops/android-release.md) | Android のリリースと署名 | 手順 |
| [android-emulator.md](ops/android-emulator.md) | エミュレータでの検証 | 手順 |
| [licenses.md](ops/licenses.md) | サードパーティライセンス表記 | 手順 |

## records — 記録（追記のみ）

| 文書 | 内容 |
|---|---|
| [implementation-review.md](records/implementation-review.md) | 実装レビューの台帳（`R` 番号の正本） |
| [implementation-progress.md](records/implementation-progress.md) | 2ノード同期までの実装記録 |
| [review.md](records/review.md) | 仕様との照合レビュー |
| [handoff-2026-11-20.md](records/handoff-2026-11-20.md) | 実用化改善の引継ぎ（2026-11-20） |
| [wiki-m0.md](records/wiki-m0.md) | Wiki M0 スパイクの記録 |

記録は消さない。判断は設計文書へ移し、記録には経緯だけを残す。新しい記録は `YYYY-MM-DD-<topic>.md` で足す。

## assets

画像・設計ソース。[assets/README.md](assets/README.md) を参照。

## 規約

1. **1トピック1ファイル。** 実装状況は `status.md` に集約し、コードの README と二重管理しない。
2. **先頭4行は `状態` / `対象` / `最終更新` / `正となる実装`。** 片方だけ直せない状態を作らない。
3. **計画は実装したら状態を書き換える。** 実装済みになった計画は `実装済み` にし、判断の経緯だけ `records/` へ送る。
4. **リンクは相対パス。** 移動したら `perl scripts/check-doc-links.pl` を通す。
5. **`Phase` を使わない。** 番号は roadmap の ID に一本化する。

## 再編の履歴

2026-11-28 にフラットな `docs/*.md` から `core/` `apps/` `ops/` `records/` `assets/` へ移動した。旧パスからの主な対応:

| 旧 | 新 |
|---|---|
| `docs/specification.md` | `docs/core/requirements.md` |
| `docs/implementation-review.md` | `docs/records/implementation-review.md` |
| `docs/implementation-handoff.md` | `docs/records/handoff-2026-11-20.md` |
| `docs/android-node.md` | `docs/apps/node/requirements.md` |
| `docs/mote-phase3.md` | `docs/apps/mote/voice.md` |
| `docs/wiki-implementation-status.md` | `docs/apps/wiki/status.md` |
| `docs/ui-copy-audit.md` | `docs/ops/ui-copy.md` |
| `docs/harmion-admin-design.md` | `docs/ops/admin.md` |
| `docs/*.png` | `docs/assets/{mote,wiki,admin}/` |
| `docs/*.fig`, `docs/*.html` | `docs/assets/source/` |
