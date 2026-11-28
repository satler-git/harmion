# Harmion

Harmion は、アプリから独立した個人データ基盤です。データと権限は自分のノードに置いたまま、Android アプリや Linux の CLI/管理 UI から同じデータを使います。ノード間は同期し、必要な場合だけ Cloudflare Worker を中継に使います。

現在は Rust 実装です。Android ノード、Mote、Linux native、Cloudflare Worker/DO まで動作し、Worker は `hub.satler.dev` にデプロイ済みです。機能ごとの実装範囲は[実装全体レビュー](docs/records/implementation-review.md)を参照してください。

## 配布物

| 対象 | 説明 | 入手 |
|---|---|---|
| Harmion node | Android 10+。ノード本体・管理UI・接続承認 | Obtainium / GitHub Releases |
| Mote | Android 10+。Capture アプリ | Obtainium / GitHub Releases |
| native | Linux。`harmiond`、`harmion` CLI、`harmion-admin`、`mote` CLI | flake / `harmion-update` |
| Worker | Cloudflare Workers + Durable Objects。任意の中継 | [`deploy/cloudflare`](deploy/cloudflare/README.md) |

## Android（Obtainium）

[Obtainium](https://obtainium.imranr.dev/) で GitHub Releases を追跡します。下のボタンは Add App 画面を開き、このリポジトリ用の filter まで入力済みです。内容を確認して追加してください。Obtainium が未導入の場合は導入先を案内するページが開きます。

| アプリ | Obtainium に追加 |
|---|---|
| Harmion node | <a href="https://apps.obtainium.imranr.dev/redirect?r=obtainium://app/%7B%22id%22%3A%22dev.satler.harmion%22%2C%22url%22%3A%22https%3A%2F%2Fgithub.com%2Fsatler-git%2Fharmion%22%2C%22author%22%3A%22satler-git%22%2C%22name%22%3A%22Harmion%22%2C%22additionalSettings%22%3A%22%7B%5C%22filterReleaseTitlesByRegEx%5C%22%3A%5C%22%5Enode-v%5C%22%2C%5C%22apkFilterRegEx%5C%22%3A%5C%22%5Eharmion-node-%5C%22%2C%5C%22versionExtractionRegEx%5C%22%3A%5C%22v%28%5B0-9%5D%2B%5C%5C%5C%5C.%5B0-9%5D%2B%5C%5C%5C%5C.%5B0-9%5D%2B%29%5C%22%2C%5C%22matchGroupToUse%5C%22%3A%5C%221%5C%22%2C%5C%22includePrereleases%5C%22%3Afalse%7D%22%7D"><img src="docs/assets/obtainium-badge.png" alt="Add Harmion node to Obtainium" width="180"></a> |
| Mote | <a href="https://apps.obtainium.imranr.dev/redirect?r=obtainium://app/%7B%22id%22%3A%22dev.satler.mote%22%2C%22url%22%3A%22https%3A%2F%2Fgithub.com%2Fsatler-git%2Fharmion%22%2C%22author%22%3A%22satler-git%22%2C%22name%22%3A%22Mote%22%2C%22additionalSettings%22%3A%22%7B%5C%22filterReleaseTitlesByRegEx%5C%22%3A%5C%22%5Emote-v%5C%22%2C%5C%22apkFilterRegEx%5C%22%3A%5C%22%5Emote-%5C%22%2C%5C%22versionExtractionRegEx%5C%22%3A%5C%22v%28%5B0-9%5D%2B%5C%5C%5C%5C.%5B0-9%5D%2B%5C%5C%5C%5C.%5B0-9%5D%2B%29%5C%22%2C%5C%22matchGroupToUse%5C%22%3A%5C%221%5C%22%2C%5C%22includePrereleases%5C%22%3Afalse%7D%22%7D"><img src="docs/assets/obtainium-badge.png" alt="Add Mote to Obtainium" width="180"></a> |

ボタンを使わず手動で追加する場合の設定は次のとおりです。ボタンで失敗する場合も、Obtainium の Add App に `https://github.com/satler-git/harmion` を貼り、この表を設定してください。Obtainium の filter は **APK ファイル名**に対する正規表現で、パッケージ名では絞れません。同じリポジトリに node と Mote の Release があるため、release title と APK filename の両方で分けます。

| 設定 | Harmion node | Mote |
|---|---|---|
| App Source URL | `https://github.com/satler-git/harmion` | 同じ |
| Filter release titles by regex | `^node-v` | `^mote-v` |
| Filter APKs by regex | `^harmion-node-` | `^mote-` |
| Trim version string by regex | `v([0-9]+\.[0-9]+\.[0-9]+)` | 同じ |
| Match group | `1` | `1` |
| Include prereleases | off | off |

パッケージ名は、インストール済みアプリの確認に使えます。

- Harmion node: `dev.satler.harmion`
- Mote: `dev.satler.mote`

初回起動後のノード設定・参加は[Androidノードの要件定義](docs/apps/node/requirements.md)、Mote の使い方は[Mote の設計・起動手順](docs/apps/mote/design.md)を参照してください。新しい Release は `node-v*` / `mote-v*` タグで作られます。

## Linux native

native のバイナリは GitHub Release に置きません。flake が唯一の配布元です。

インストール済みの `harmion-update` を使う場合:

```sh
harmion-update --version v0.1.1
harmion-update --rollback
```

clone から初めて入れる場合:

```sh
./scripts/harmion-update --version v0.1.1
```

`~/.local/bin` に symlink し、`~/.local/share/harmion/profiles/<tag>` を GC root として保持します。dotfiles や home-manager は介しません。`--systemd` で unit を配置し、`--restart` で `harmion@main.service` を再起動できます。

作業ツリーから入れる場合は `--from-local .` を使う。git checkoutでは**git管理下のファイルだけ**をビルドし、`target/` などのignored artifactはNix storeへコピーしない。未コミット・untrackedの変更は本番入力にしない。

flake から直接ビルドする場合:

```sh
nix build .#default
nix build .#harmiond .#harmion .#harmion-admin .#mote
```

日常運用は[Linuxユーザーサービスと日常運用](docs/ops/linux-service.md)、複数ノードの起動は[native 複数ノード同期](docs/ops/native-quickstart.md)を参照してください。

## Cloudflare Worker

Worker は常設の中継・同期先を使う場合だけ必要です。通常の Android / Linux 利用だけなら不要です。デプロイ、secret、Access の設定は[`deploy/cloudflare/README.md`](deploy/cloudflare/README.md)にあります。

本番 endpoint: `https://hub.satler.dev`

## ドキュメント

一覧は [docs/README.md](docs/README.md)。入口だけここに置く。

- [ロードマップ](docs/roadmap.md) — 決定事項、作業項目、ゲート
- [現時点の仕様](docs/core/requirements.md)
- [Architecture](docs/core/architecture.md)
- [Androidノードの要件](docs/apps/node/requirements.md)
- [Mote の設計](docs/apps/mote/design.md)
- [Wiki の要件](docs/apps/wiki/requirements.md)
- [Linuxユーザーサービスと日常運用](docs/ops/linux-service.md)

## 開発

Nix を使うと、rustc/cargo/clippy/rustfmt（wasm32 std 込み）、管理UI用の node/npm、検証用の python3、C toolchain が揃います。`flake.nix` と `flake.lock` は git の追跡対象に含めてください。

```sh
nix develop
nix develop .#android
```

```sh
npm ci --prefix apps/admin/ui
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
```

native は GitHub Release のバイナリを作らず、flake からビルドします。Android は `node-v*` / `mote-v*`、Worker は `v*` タグでリリースします。詳細は[CI/CD 計画](docs/ops/ci-plan.md)と[ship 戦略](docs/ops/shipping.md)を参照してください。

## ライセンス

[LICENSE](LICENSE)

配布物に同梱するサードパーティ通知の生成と確認は[サードパーティライセンス表記](docs/ops/licenses.md)を参照してください。
