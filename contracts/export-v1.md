# Harmion Export v1

Export v1 は所有者によるデータ持ち出し用の形式。対象範囲は `current-conflicts-and-deletions`。本文、現在値、未解決の候補、保持されている削除記録を読み取れる。ノード鍵、アプリ認可、資格情報、同期カーソル、編集トークンは出力範囲外。一般の過去版と運用バックアップ、Harmion へのインポートは別の契約で扱う。

## ディレクトリ

- `manifest.json` は `format: "harmion-export"`、`version: 1`、`scope: "current-conflicts-and-deletions"`、`metadata: "metadata.jsonl"`、`records`、`body_files` を持つ。件数はそれぞれメタデータ行と本文ファイルの数。
- `metadata.jsonl` は1行に1つの JSON オブジェクトを持つ UTF-8 ファイル。
- `bodies/<content_id>.txt` は通常本文の UTF-8 バイト列。改行の付加や正規化を行わない。

CLI は未作成の出力先を所有者用の権限で作る。すべてのデータを書き終えて転送終端を検証した後にマニフェストを公開する。中断したディレクトリには完成したマニフェストがなく、別の未作成ディレクトリでやり直せる。

## メタデータ行

Content ID と更新 ID は32バイトを小文字の16進で表した文字列。時刻は符号付き Unix epoch UTC マイクロ秒の10進文字列。各行は `type` と `content_id` を持つ。

| type | 記録する内容 |
|---|---|
| `content` | `life` は `live`、`deleted`、`conflict` のいずれか。 `last_edited_at` は通常表示に選ばれる項目版から算出した時刻 |
| `existence_head` | 現在の存在グラフの末端ごとの `revision` |
| `body` | 通常本文の相対ファイル名 `file` と選択された `revision`。生存・削除競合中の Content に出力 |
| `field` | 通常表示で選択された属性・kind の `field`、`value`、`revision`。属性の unset と kind の非所属も含む |
| `candidate` | 競合中の項目の現在候補を版ごとに出力。`field`、`value`、`revision` と通常表示に選ばれる版かどうかの `selected` を持つ |
| `deletion` | 受理済みの削除操作の `revision` と、その操作が対象とした存在グラフの版 ID の配列 `basis`。後で復元された削除も含む |

`revision` は `id`、`edited_at`、`action` を持つ。`field` の `type` は `body`、`attribute`、`kind`。属性は `namespace` と `key`、kind は `name` を持つ。`value` の `type` は `body`、`attribute`、`kind`、`absent`。 `absent` 以外には実値を持つ `value` がある。属性内の正確な数値が必要な読み手は、精度を保つ JSON パーサーを使う。版 ID の参照先には、この出力範囲外の版もあり得る。

## Native App API

`POST /app/v1/export` は空の本文と管理者資格情報を使う。削除済み・非公開の競合候補も含むため、`inspect` と同じ全権限を必要とする。Export 開始時の受理ログ位置を固定し、Content ごとの取得で資格情報と権限を再検査する。

ネイティブ daemon は同時に1件の Export を受け付ける。既に転送中なら HTTP 429。転送中に読み取りが失敗した場合や資格情報が失効した場合は、転送を終了する。

レスポンスは `application/x-ndjson`。先頭の `header` は `format`、`version`、`scope` を持ち、次にメタデータと同じデータ行が続く。本文行だけは `file` に代えて `text` に本文を載せる。末尾の `end` はデータ行数 `records` を持つ。クライアントは1行あたり最大8 MiBを読み、`end` と行数、レスポンスの終端を確認する。
