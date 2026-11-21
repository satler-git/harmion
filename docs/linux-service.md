# Linux: user service と日常運用

Linux の systemd **ユーザー**サービス用の任意の導入手順。リポジトリの
[unit template](../deploy/systemd/harmion@.service) を使う。以下の導入・停止・起動コマンドは
**所有者が内容を確認して自分で実行する手順**であり、ビルドや検証スクリプトは実行しない。
root、システム全体のサービス、独自ログイン基盤は必要ない。

## 配置と instance

- 実行ファイル: `~/.local/bin/harmiond`、`harmion`、`harmion-admin`
- unit: `~/.config/systemd/user/harmion@.service`
- instance `main` のノード: `~/.local/share/harmion/main`
- サービス名: `harmion@main.service`

instance は `main` や `work` のような単純な英数字・ハイフン・アンダースコアの名前にする。
`/`、`.`、`..` や任意のパスを instance に渡さない。この template は systemd の `%h`
（ユーザーのホーム）と `%i`（instance）を使い、シェルや現在のディレクトリには依存しない。
既存ノードが別の場所にある場合、勝手に移動・コピーして起動せず、絶対パスを明示した
所有者用の unit に変更してから確認する。ノードディレクトリは所有者限定でなければならない。

## Release artifact の作成

リポジトリのルートで、開発用依存が利用できる環境から実行する。

```sh
npm ci --prefix apps/admin/ui
cargo build --release --workspace
```

管理 UI は Cargo のビルド時に組み込まれる。Node.js/npm はビルド時に必要だが、
完成した管理アプリの実行時には不要。実行先と同じアーキテクチャ・互換ライブラリ環境で
ビルドし、別ホストでの動作は別途確認する。初期段階で汎用の静的バイナリ互換性は保証しない。

## 所有者による初回導入

初回の新規配置の例。既存の実行ファイル・unit がある場合は、上書き前に下の更新手順を読む。

```sh
umask 077
install -d -m 700 "$HOME/.local/bin" "$HOME/.local/share/harmion" \
  "$HOME/.config/systemd/user"
install -m 700 target/release/harmiond "$HOME/.local/bin/harmiond"
install -m 700 target/release/harmion "$HOME/.local/bin/harmion"
install -m 700 target/release/harmion-admin "$HOME/.local/bin/harmion-admin"
install -m 600 deploy/systemd/harmion@.service \
  "$HOME/.config/systemd/user/harmion@.service"

# 未作成の directory を指定する。既存ノードには init を実行しない。
"$HOME/.local/bin/harmiond" init "$HOME/.local/share/harmion/main"
```

既存クラスタへ追加する場合は `init` に `--cluster CLUSTER_ID` を指定する。
参加承認と `join`、iroh relay/discovery、OAuth の明示的な設定は
[native の手順](native-quickstart.md#起動と参加)に従う。unit は保存済み設定を使う。
初期設定は n0 公開リレー・探索を使う開発用構成であり、本番の到達性保証ではない。
OAuth と管理 UI の CA は別途所有者が確認・設定する。このサービスは CA の登録や
管理 UI の自動起動をしない。

内容を確認した後、**所有者が任意で**ユーザーサービスを登録・起動する。

```sh
systemd-analyze --user verify "$HOME/.config/systemd/user/harmion@.service"
systemctl --user daemon-reload
systemctl --user enable --now harmion@main.service
systemctl --user status harmion@main.service
```

unit に初期化・復元・鍵生成の `ExecStartPre` はない。初期化忘れ、権限不正、鍵紛失、破損は
起動失敗のまま扱い、自動で新しい identity に置き換えない。直接起動した daemon とサービスを
同じノードで同時に動かさない。排他 lock により拒否される。

## ライフサイクルとログ

`Type=simple` は foreground daemon の開始を追跡するもので、ネットワーク到達性や
同期完了の readiness 通知ではない。異常終了時は5秒後に再試行し、60秒間に3回の開始制限を設ける。
設定不正を無制限の高速再起動で隠さない。原因を直した後、必要なら所有者が
`systemctl --user reset-failed harmion@main.service` を実行してから再起動する。

停止は SIGTERM を送り、最大60秒待つ。超過時は systemd の既定の強制終了処理へ進む。
正常停止や所有者の停止操作を `Restart=always` で打ち消す設定にはしない。
`UMask=0077` と `NoNewPrivileges=yes` を設定するが、同一OSユーザー内の悪意あるプログラムを
隔離する保証ではない。未検証の filesystem/network namespace 制限は追加していない。

```sh
journalctl --user -u harmion@main.service -n 100 --no-pager
"$HOME/.local/bin/harmion" --node "$HOME/.local/share/harmion/main" sync-status
```

標準出力の公開登録・ready JSON と標準エラーの診断ログは journal に記録される。
credential、編集トークン、秘密鍵を unit の引数・環境変数・ログへ追加しない。
ログを他人に渡す場合は Node ID・接続情報などの公開メタデータも確認する。
`sync-status` はローカル保存済みの観測を読み、ネットワーク同期を起こさない。
保存成功はローカル永続化、受領確認はそのPeerで過去に確認した保存、到達性は現在の接続状態であり別物。
Android が電池・ネットワークの都合で停止していても正常な運用であり、停止・退役を意味しない。

ユーザーサービスは通常ユーザーマネージャーの寿命に従い、ログアウト後は停止する場合がある。
`enable` だけで24時間常駐を保証しない。ログアウト後やログイン前にも動かす linger は、
消費電力・ホストポリシーを確認して所有者が別途判断する設定であり、この手順では変更しない。
Linux/Android 端末が常時稼働することを複製の成立条件にしない。

## 更新・バックアップ・失敗時の対応

1. 新しい release をビルドし、検証結果を確認する。旧バイナリも別の場所に保管する。
2. 所有者がサービスを停止する。管理アプリなどの操作も止め、daemon が終了したことを確認する。
3. **停止済みのノード directory 全体**を新しい所有者限定のバックアップ先へコピーする。
   DB だけでなく sidecar・鍵・設定・ローカル状態を含める。稼働中の単純ファイルコピーは
   coherent backup と扱わない。秘密を含むため共有・クラウド公開しない。
4. 停止中に実行ファイルを置き換え、必要なら unit を更新する。unit 変更時だけ
   `systemctl --user daemon-reload` を実行し、サービスを起動してログと状態を確認する。

```sh
systemctl --user stop harmion@main.service
systemctl --user is-active harmion@main.service  # inactive を確認（終了コード非0は想定）
# daemon 終了と他の同一ノードプロセスがないことを確認してからコピーする。
umask 077
install -d -m 700 "$HOME/.local/share/harmion-backups"
# 宛先は未作成の名前にする。既存バックアップへ混ぜない。
cp -a "$HOME/.local/share/harmion/main" \
  "$HOME/.local/share/harmion-backups/main-before-upgrade-UNIQUE"
# バックアップの所有者・mode とコピー完了を確認してから更新する。
install -m 700 target/release/harmiond "$HOME/.local/bin/harmiond"
install -m 700 target/release/harmion "$HOME/.local/bin/harmion"
install -m 700 target/release/harmion-admin "$HOME/.local/bin/harmion-admin"
systemctl --user start harmion@main.service
```

起動時に schema migration が行われることがある。**バイナリだけ旧版へ戻してよいとは限らない**。
旧版が新しい schema を扱えるかを確認し、失敗中に繰り返し書き換えない。
古いバックアップを元の場所へ戻し、同じ Node ID のまま再開する方法を rollback として使わない。
相手の受領済み cursor と食い違って更新が届かなくなる可能性がある。
バックアップから復元する場合は対応するバイナリで `harmiond restore BACKUP_DIR NEW_DIR` を使い、
**新しい Node ID・鍵・ログ世代で**復元した後、有効なノードから再承認を受けて `join` / `sync` する。
詳細と有効なノードがない場合の制限は[復元契約](native-quickstart.md#古いノードバックアップからの復元)を参照。
復元先は新しい instance を使うか、unit の絶対パスを明示的に変更する。古いノードを同時に起動しない。
Export は可読なデータ搬出であり、この秘密情報を含む運用バックアップと同じではない。

## この artifact の検証範囲

```sh
nix develop --command python3 design_checks/linux_service.py
```

検証は unit の固定設定と危険な自動操作の不在を検査する。`systemd-analyze` が利用できる場合は、
一時 directory に実行ファイルの代役を置き、検証用に `ExecStart` のみ差し替えて構文を確認する。
実際のユーザーサービスの登録・起動・停止、ホストへのインストールは行わない。
この静的検証だけで systemd 配下の実ライフサイクル、ログアウト、実kernel suspend、
実ネットワーク切替、リレーの本番可用性を検証済みとはしない。daemon 自体の再起動・同期の
プロセス検証は[native の検証手順](native-quickstart.md#検証)を別途参照する。
