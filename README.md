# Harmion

一人のユーザーが所有する、アプリから独立した個人データ基盤。

Rustで初期実装を進めています。Content・SQLite保存・App HTTPS API・daemon/CLI・HTTPS/irohのPeer同期、署名付き参加・退役・却下証明の伝播を実装しています。nativeではQuery/Subscribe、所有者向けExport、専用CAを使うブラウザOAuth、ローカルInterpreterの登録・承認・Provider WebSocket・呼び出し仲介も利用できます。初回接続はアプリ開始→管理UI「接続」承認→アプリのポーリング完了に統合し、OAuth grantとProvider登録を同じ承認で確定します。5つの独立プロセスで中継・分断・再起動・周期同期を検証し、管理情報の伝播・保留・所有者回復は別の実プロセステストで確認しています。Workers/DOはローカルworkerdまで実装・検証済みで、実Cloudflareには未配置です。各機能の範囲は[実装全体レビュー](docs/implementation-review.md)を参照してください。

ネイティブの App HTTPS と Rust SDK は、固定スナップショットの Query、初回取得と変更通知をつなぐ Subscribe、購読キャッシュを提供します。所有者向けの `sync-status` では、最後の自発同期の成功・失敗と、保存した更新のPeerごとの受領確認を取得できます。ローカル保存成功・過去の受領確認・現在の到達性は区別します。

- [Androidノードの要件定義・実装指針](docs/android-node.md)
- [Androidノードの設計・実装状態](docs/android-design.md)
- [Mote（Android Captureアプリ）の設計・起動手順](docs/mote-design.md)

Android版は、Rust CoreをJNIで埋め込んだ単一プロセスのノードアプリ（管理UI・接続承認Activity・Bound Service）と、
外部アプリ向けSDK（AIDLで発見・承認、pinned HTTPSでデータ操作）で構成します。同期は`harmion-runtime`が所有し、
Wi-Fi・従量制・バッテリー・ローミングのポリシーとWorkManagerを組み合わせます。ビルドは`nix develop .#android`を使います。

- [Linuxユーザーサービスと日常運用](docs/linux-service.md)

- [現時点の仕様と設計方針](docs/specification.md)
- [過去の議論との照合・整合性レビュー](docs/review.md)
- [実装全体レビュー・仕様との差異・暫定対応](docs/implementation-review.md)
- [実用化改善の引継ぎ・停止位置と再開手順](docs/implementation-handoff.md)
- [参加・退役の設計判断](docs/membership-design.md)
- [署名付き参加・退役・却下証明の契約](contracts/membership-v1.md)
- [内部表現と同期の初期設計](docs/internal-design.md)
- [ArchitectureとRustパッケージ構成](docs/architecture.md)
- [取得・編集・Subscribe・OAuthの契約](docs/app-api.md)
- [Wikiアプリの要件定義（草案）](docs/wiki-app-requirements.md)
- [Wiki GUIの刷新とフレームワーク判断](docs/wiki-ui-refresh.md)
- [設計の実行可能な意味モデル](design_checks/model_check.py)
- [2ノード同期までの実装・レビュー記録](docs/implementation-progress.md)
- [更新レコードの保存・転送形式v1](contracts/record-v1.md)
- [App保存要求・認可・ローカル状態](contracts/app-save-v1.md)
- [HTTPS Peerと直接承認の契約](contracts/peer-v1.md)
- [Interpreter v1の登録・呼び出し契約](contracts/interpreter-v1.md)
- [所有者認証の将来設計メモ](docs/owner-authentication-memo.md)

`crates/domain` はI/Oや非同期ランタイムに依存しないRustライブラリです。エラー型の定義には `thiserror` を使います。型で区別したID、create/edit/delete/keep/restore、項目と存在の候補、通常値選択、重複受理と不正参照の検証を扱います。Python意味モデルの10シナリオに対応する到着順テストと、追加の境界テストを用意しています。

Nixを使う場合、`nix develop`でrustc/cargo/clippy/rustfmt（wasm32 std込み）、管理UI用のnode/npm、検証スクリプト用のpython3、C toolchainが揃う。`flake.nix`・`flake.lock`はgitの追跡対象に含める必要がある。Nixを使わない場合は、同じものを個別に用意する。

```sh
npm ci --prefix apps/admin/ui
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

`harmion`・`harmiond`・`harmion-admin` の引数・サブコマンドは `--help` で確認できます。引数エラーは終了コード2、実行時エラーは終了コード1を返します。daemonのログは標準エラー出力に書き、`RUST_LOG=harmiond=debug,harmion_net_native=debug` で同期の再試行間隔・転送件数・接続エラーも確認できます。JSONやPEMの結果は標準出力に返します。

`Content` はID・本文・名前空間付き属性・kindsをまとめた通常値です。`Graph::snapshot(id)` は存在状態、固定basis、値ごとにまとめた競合候補と版IDも持つ、所有された `ContentSnapshot` を返します。同じ値の版IDを保持するため、その値を編集しても他の競合値を解決しません。snapshotは後から更新を受理しても変わりません。

削除済みデータも内部snapshotでは保持し、`visible_content()` は削除済みなら `None`、削除競合なら通常値を返します。属性のunsetとkindのfalseは通常値から省きますが、編集元の版情報は保持します。snapshotは認可前の内部表現です。node層はこれを直接公開せず、認可・情報制限を行い、不透明な編集トークン付きのReadViewへ変換します。

属性は標準で `AttributeValue` を使い、null・真偽値・数値・文字列・配列・オブジェクトを表します。不存在はnullとは別です。数値は正確な10進数で、`1`・`1.0`・`10e-1` を等値、負のゼロをゼロとして扱います。係数は任意桁、入力指数と正規化後の指数はi64範囲に制限します。配列は順序・重複を保持し、オブジェクトはキー順によらず比較します。record codecでは重複キー・非canonical入力を拒否し、1 MiB・深さ64・コンテナ要素合計32,768の上限を適用します。`Graph<V>` の型引数は意味モデル等のテスト用に残しています。

IDは32バイトの型として扱い、乱数生成はホスト側の責任です。現在のGraphは全履歴をメモリ上に保持し、読出し時に候補を再計算します。SQLite adapterは正本レコードと受理ログ・Peerカーソルを永続化しますが、索引はまだ持ちません。

実ファイルのSQLite DBとTCP/TLSでページ転送・再送・再起動・競合収束をテストしています。承認済み証明書とNode IDを照合し、退役は既存接続にも適用します。nodeのアプリ操作ではcredential・失効・期限・変更前後の権限を同じトランザクションで検査し、固定編集トークンとrequest_idの保存結果を保持します。独立2プロセスの検証は`cargo build --workspace`後に`python3 design_checks/two_process_sync.py`で実行できます。ネットワークテストにはlocalhostの待受け権限が必要です。未決定事項を確定仕様として扱わず、仕様・内部設計・レビューを併せて参照してください。

モデル検証はPython 3.10以降で `python3 design_checks/model_check.py` を実行できます。外部パッケージは不要です。

5プロセスのデータ同期検証は、`cargo build --workspace`後に`python3 design_checks/multi_process_sync.py`で実行します。管理情報の伝播・保留・回復は`python3 design_checks/membership_process_sync.py`で実行します。iroh経路は`cargo build -p harmion-net-native --example local_relay`後に各スクリプトへ`--iroh`を付けます。公開探索を使わず、テストが起動するローカルリレーを使用します。
