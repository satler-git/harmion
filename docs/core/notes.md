# 実装メモ

README から移した実装寄りの詳細です。仕様として確定していない内容も含むため、契約は `docs/core/requirements.md`、`docs/core/app-api.md`、`contracts/` を優先してください。

## ランタイムと API

- native では Query/Subscribe、所有者向け Export、専用 CA を使うブラウザ OAuth、ローカル Interpreter の登録・承認・Provider WebSocket・呼び出し仲介を利用できる。
- 初回接続は「アプリ開始 → 管理UIの『接続』承認 → アプリのポーリング完了」に統合し、OAuth grant と Provider 登録を同じ承認で確定する。
- 5 つの独立プロセスで中継・分断・再起動・周期同期を検証する。管理情報の伝播・保留・所有者回復は別の実プロセステストで確認する。
- Workers/DO はローカル workerd で実装・検証済み。2026-11-26 に `hub.satler.dev` へ初回デプロイした。Access/TLS/secret の運用は `deploy/cloudflare/README.md` を参照。
- native の App HTTPS と Rust SDK は、固定スナップショットの Query、初回取得と変更通知をつなぐ Subscribe、購読キャッシュを提供する。`sync-status` では最後の自発同期の成功・失敗と、Peer ごとの更新受領確認を取得できる。ローカル保存成功・過去の受領確認・現在の到達性は区別する。

## Domain model

- `Content` は ID・本文・名前空間付き属性・kinds をまとめた通常値。`Graph::snapshot(id)` は存在状態、固定 basis、値ごとにまとめた競合候補と版 ID を持つ `ContentSnapshot` を返す。
- 同じ値の版 ID を保持するため、その値を編集しても他の競合値を解決しない。snapshot は後から更新を受理しても変わらない。
- 削除済みデータも内部 snapshot では保持し、`visible_content()` は削除済みなら `None`、削除競合なら通常値を返す。属性の unset と kind の false は通常値から省くが、編集元の版情報は保持する。
- snapshot は認可前の内部表現。node 層はこれを直接公開せず、認可・情報制限を行い、不透明な編集トークン付きの ReadView へ変換する。
- 属性は標準で `AttributeValue` を使い、null・真偽値・数値・文字列・配列・オブジェクトを表す。不存在は null とは別。数値は正確な 10 進数で、`1`・`1.0`・`10e-1` を等値、負のゼロをゼロとして扱う。
- 係数は任意桁、入力指数と正規化後の指数は i64 範囲に制限する。配列は順序・重複を保持し、オブジェクトはキー順によらず比較する。record codec では重複キー・非 canonical 入力を拒否し、1 MiB・深さ 64・コンテナ要素合計 32,768 の上限を適用する。
- `Graph<V>` の型引数は意味モデル等のテスト用に残している。
- ID は 32 バイトの型として扱い、乱数生成はホスト側の責任とする。現在の Graph は全履歴をメモリ上に保持し、読出し時に候補を再計算する。SQLite adapter は正本レコードと受理ログ・Peer カーソルを永続化するが、索引はまだ持たない。

## 検証

```sh
cargo build --workspace
python3 design_checks/model_check.py
python3 design_checks/two_process_sync.py
python3 design_checks/multi_process_sync.py
python3 design_checks/membership_process_sync.py
```

実ファイルの SQLite DB と TCP/TLS でページ転送・再送・再起動・競合収束をテストする。承認済み証明書と Node ID を照合し、退役は既存接続にも適用する。node のアプリ操作では credential・失効・期限・変更前後の権限を同じトランザクションで検査し、固定編集トークンと request_id の保存結果を保持する。ネットワークテストには localhost の待受け権限が必要。

iroh 経路は次の例をビルドした後、各スクリプトへ `--iroh` を付ける。公開探索を使わず、テストが起動するローカルリレーを使用する。

```sh
cargo build -p harmion-net-native --example local_relay
```

詳細は[内部設計](domain.md)、[実装全体レビュー](../records/implementation-review.md)、[Architecture](architecture.md)を参照。
