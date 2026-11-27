# Mote CLI

Mote の `capture` Content を Linux の端末から読む・書く・処理する CLI。Android 版と同じ Harmion の Content を扱う。

状態: V1。短文 Capture、outbox、接続・credential更新、一覧、詳細、検索、アーカイブ、ピン留めまで。長文、削除、HUD、音声は未実装。

## ビルド

```sh
nix develop
cargo build -p mote
target/debug/mote --help
```

flake からは `nix build .#mote` で `mote` バイナリをビルドできる。

## 接続

既定では `~/.local/share/harmion/main` のノードディレクトリを使う。`--node` か `MOTE_NODE` でディレクトリまたはノード名を指定する。`main` のような裸の名前は常に Harmion のノードルート配下として解決し、カレントディレクトリの `main/` では shadow されない。相対ディレクトリは `./node` のように separator を入れる。

```sh
mote --node ~/.local/share/harmion/main list
MOTE_NODE=main mote list
```

### 認可

- 初回は `mote connect` で所有者に接続を承認してもらい、`capture` の Read / Write だけを持つaccess tokenとrefresh tokenを `<node>/mote-credentials.json`（0600）へ保存する。以後のコマンドは保存済みcredentialを使う。
- access tokenは最長10分。期限が近づくとrefresh tokenで自動更新し、rotate後のrefresh tokenを先に保存する。
- `mote connect` はブラウザを自動で開かない。表示されたURLまたはHarmion Adminの「接続」画面で承認する。
- `--token-file FILE` / `MOTE_TOKEN` は既存のaccess tokenを一時的に使う経路。argvの `--token` は廃止した（プロセス一覧と shell history に残るため）。token と `--node` は同じ信頼境界として扱う。他者から渡されたノードディレクトリの `connection.json` に token を送らない。
- `--owner` はノードディレクトリの `admin.secret` を使う明示的なローカル管理者フォールバック。capture-only ではなく、実行時に警告を出す。Release の通常経路にしない。
- ノードの Read / Write Filter は **Content 全体**に対する Kind フィルタで、field 単位の制限ではない。`capture` を含む Content への token は、その Content の他の Kind や属性も編集できる。CLI の builder は `capture.*` の field だけを送るが、capture-only は field-level の境界ではない。
- `MOTE_NODE` / `MOTE_TOKEN` は非 UTF-8 や空文字ならエラーにする。空 token で所有者経路へフォールバックしない。

### 接続とcredential

`mote connect` はnodeの `connect-start` にcapture-only grantを出し、`connect-complete` で得たtokenを `<node>/mote-credentials.json` に保存する。所有者は表示されたURL、またはHarmion Adminの「接続」画面で承認する。Mote自身は `admin.secret` を読まない。

```sh
mote connect --timeout 600
mote auth status
mote auth status --json
mote auth logout
```

- credentialはnodeディレクトリごとに0600。同期しない。
- access tokenの期限が近づくと、`harmion-client` のOAuth refreshで自動更新する。refresh tokenはrotateされるため、新しいaccess tokenを使う前に保存する。
- nodeが `oauth_origin` を公開していない場合、connectは成功するが自動更新できない。期限後は `mote connect` をやり直す。
- `--token-file` / `MOTE_TOKEN` は保存済みcredentialより優先される。`--owner` はさらに明示的な管理者フォールバック。優先順は token > `--owner` > 保存済みcredential。
- `mote connect` はnodeに接続要求を作るだけで、承認は所有者が行う。
- `mote auth logout` はnodeの `revoke-self` でgrantとOAuth系列を失効させ、成功後にローカルcredentialを消す。ノードに到達できない場合はローカルcredentialを残して再試行できる。

## コマンド

以下は capture-only token を設定済みの例。初回は `mote connect` を実行する。

```sh
# 初回接続（所有者がHarmion Admin > 接続で承認）
mote --node ~/.local/share/harmion/main connect
mote auth status
mote auth logout

# 保存済みcredentialを使う
mote q "あとで調べる"

# 既存access tokenを一時利用する場合
mote --token-file ~/.config/mote/token list
export MOTE_TOKEN="$(tr -d '\n' < ~/.config/mote/token)"  # shell historyに実値を残さない
printf '標準入力から\n' | mote q -
mote q -                      # TTY でも明示的に stdin を読む

# 送信待ちの確認と再送
mote pending
mote pending --json
mote retry
mote retry 7f3a9c1e
mote forget 7f3a9c1e

# 返信
mote q --reply-to 7f3a9c1e "返信"

# 一覧（Inbox、古い順、20件）
mote list
mote list --all --limit 0
mote list --archived
mote list --pinned
mote list --reverse
mote list --search "Wi-Fi"

# 検索（既定は Inbox。--all でアーカイブを含む全件）
mote search "Wi-Fi"
mote search "Wi-Fi" --all
mote search "Wi-Fi" --archived

# 本文だけを出力（パイプ向け）
mote show 7f3a9c1e
mote show 7f3a9c1e | wl-copy

# メタデータ付き
mote show 7f3a9c1e --meta

# 処理
mote archive 7f3a9c1e 0123abcd
mote unarchive 7f3a9c1e
mote pin 7f3a9c1e
mote unpin 7f3a9c1e
```

`--owner` はローカル管理者フォールバック:

```sh
mote --owner --node ~/.local/share/harmion/main list
```

Content ID は完全な64桁、または一意な4桁以上の prefix を受け付ける。prefix が複数に一致した場合は候補を表示して失敗する。`retry` の ID も同じ prefix を受け付ける。

## ローカル outbox

`q` は送信前に `$XDG_STATE_HOME/mote/requests`（未設定なら `~/.local/state/mote/requests`）へ送信要求を保存する。ディレクトリは 0700、ファイルは 0600。`MOTE_STATE_DIR` で場所を変更できる。

- 保存してからノードへ送る。認証失敗、ノード停止、送信タイムアウト、`--reply-to` の解決失敗でも本文はoutboxに残る。
- 成功したらoutboxから削除する。削除に失敗した場合は警告を出し、`retry` で同じ要求を再送できる。
- `content_id` と `request_id` はoutbox作成時に固定する。ノードがcommitした後に応答だけ失われた場合、`retry` は同じrequest IDを送るため、ノードのreceiptがある限り重複しない。
- `pending` と `forget` は認証不要でoutboxを読む・削除する。`retry` は認証が必要。
- 形式不正なoutboxファイルは無視して警告する。壊れたファイルを `q` が再利用することはない。
- outboxはノードに紐付かない。`retry` は現在の `--node` へ送るため、同じoutboxを複数ノードで使わない。

## 出力

### 人間向け

```text
2026-11-27 12:31  7f3a9c1e  APL  購買リストの件、Wi-Fiルーターの型番を確認する。
```

- 時刻はローカル時刻（分まで）
- ID は8桁 prefix
- flags は `A` archived、`P` pinned、`L` session open、`!` 表示可能な field / deletion conflict
- 本文は先頭の非空行を72文字まで

### JSON

`--json` は `list` / `search` / `show` / `q` / `pending` / `retry` / `forget` / `connect` / `auth status` / 処理で使える。tokenは出力しない。`show --json` は次を返す。編集 token は含めない。nullable な key は `null` として必ず出す。

```json
{
  "id": "7f3a...",
  "body": "購買リストの件、…",
  "created_at_micros": 1795785061649758,
  "archived": false,
  "archived_at_micros": null,
  "pinned": true,
  "session_open": false,
  "reply_to": null,
  "kinds": ["capture"],
  "conflicted": false,
  "deletion_conflict": false
}
```

### TSV

`--tsv` は `list` / `search` / `show` で使える。ヘッダーなし。列は次の順。

```text
id	created_at_micros	archived	archived_at_micros	pinned	session_open	reply_to	body
```

本文と `reply_to` などの制御文字は空白に置換する。機械処理では `--json` を推奨する。

## 終了コード

- `0`: 成功
- `1`: 一般エラー（outbox、ID解決、競合など）
- `2`: clap のusageエラー
- `3`: ノードが返した not found
- `5`: ノードが返した認証・認可エラー（ローカルtoken設定のエラーは1）
- `6`: ノードに到達できない / timeout
- `7`: ノードが返した conflict

## 並び順

- `list` の既定: Inbox（未アーカイブ）を作成時刻の昇順、同じ時刻は ID 順
- `list --archived`: アーカイブ時刻の昇順。アーカイブ時刻がない古い項目は先頭、同じ時刻は作成時刻と ID 順
- `list --all`: 作成時刻の昇順。`A` flag でアーカイブ済みを示す
- `--reverse`: 上記の逆順
- `--limit 0`: 全件
- `--limit` で切った場合は stderr に件数を出す。空の検索語はエラー。`show --meta` は `--json` / `--tsv` と併用しない。

## 制約

- 一回の `q` は `capture` Kind の Content を1件作成する。本文は最大 960 KiB。outboxに保存してから送るため、通信失敗時も `pending` / `retry` で回収できる。ノードがcommitした後に応答だけ失われた場合は、同じrequest IDの再送で重複を避ける（ノードがreceiptを保持している場合）。
- `mote q` は引数なしで TTY のとき待ちに入らずエラーにする。明示的に stdin を読むときは `mote q -` を使う。機密本文を引数に置くと `/proc/<pid>/cmdline` と shell history に残るため、stdin を使う。
- outboxはローカルに本文を平文で保存する。ディスク暗号化や `MOTE_STATE_DIR` の権限で保護する。不要な保留は `mote forget <ID>` で削除できる。
- 長文 Capture の開始・追記・完了は未実装。
- 削除は未実装。アーカイブとピン留めだけを扱う。
- 長文セッションが開いている Content のアーカイブは拒否する。一覧・詳細・ピン留めは扱えるが、完了操作はない。
- `capture` 以外の Kind が共存する Content は、`capture` を含めば一覧に出す。アーカイブ・ピン留めは競合がなければ編集する。削除は行わない。
- 音声入力は CLI に含めない。VoxType などの外部入力を `mote q` へ渡す。
- HUD は未実装。CLI の `hud` コマンドもまだない。

## テスト

```sh
cargo test -p mote-core -p mote --offline
cargo clippy -p mote-core -p mote --offline --all-targets -- -D warnings
```

一時ノードでの確認。`--owner` はローカル確認専用で、通常運用では capture-only token を使う。

```sh
tmp="$(mktemp -d)"
target/debug/harmiond init "$tmp/node"
mote="env MOTE_STATE_DIR=$tmp/state target/debug/mote --owner --node $tmp/node"

# ノードを起動せずにoutboxへ保存されることを確認する
$mote q "offline" || true
$mote pending

target/debug/harmiond run "$tmp/node" &   # 別ターミナルでもよい
sleep 1
$mote retry
$mote pending
$mote q "確認"
$mote list
id="$($mote list --json | jq -r '.[0].id')"
$mote show "$id"
$mote archive "$id"
$mote unarchive "$id"
$mote pin "$id"
kill %1
rm -rf "$tmp"
```

接続フローの確認。所有者承認は通常Harmion Adminで行う。ローカルのenforce無効ノードなら `harmion connect-approve` でも確認できる。

```sh
tmp="$(mktemp -d)"
target/debug/harmiond init "$tmp/node"
target/debug/harmiond run "$tmp/node" &
target/debug/mote --node "$tmp/node" connect --timeout 20 >"$tmp/connect.out" 2>&1 &
cpid=$!
for i in $(seq 1 50); do grep -q '^connection:' "$tmp/connect.out" && break; sleep 0.1; done
hid="$(sed -n 's/^connection: //p' "$tmp/connect.out" | head -n1)"
target/debug/harmion --node "$tmp/node" connect-approve "$hid"
wait "$cpid"
target/debug/mote --node "$tmp/node" auth status
kill %1
rm -rf "$tmp"
```
