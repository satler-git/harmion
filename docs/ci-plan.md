# CI/CD 実行プラン（2026-11-26 決定版）

状態: 計画（未実装） / 前提: [shipping.md](shipping.md) の設計を、次の決定で具体化したもの

## 0. 決定事項

| 項目 | 決定 |
|---|---|
| Android の配布 | **GitHub Releases + Obtainium**。Play は使わない（app signing cert のピン問題を避ける） |
| native の配布 | `harmion-update` が GitHub のタグから **`nix build`** し、`~/.local/bin` に symlink する。store path は profile を `nix-store --add-root` で GC root にして保持する。**musl は使わない**（非 Nix 環境向けの static が要る場合だけ任意で追加） |
| 更新 | `harmion-update` を再実行。version ごとの profile を残し、`--rollback` で前の profile に戻す |
| Nix | `nix build .#harmiond` などを必須にする。インストールも Nix ビルドを使うが、dotfiles は介さない |
| Worker | **初期は staging を作らない**。prod のみ、Environment 承認 + smoke + 前 version 保持。DO migration を入れる回だけ staging を一時構築する |
| 秘密 | 1Password の専用 vault + read-only service account。GitHub には OP token / Cloudflare token / Cachix token だけ |
| モノレポ | **分割しない**。ただし CI の前に 3 つの結合をほどく（後述） |

## 1. 成果物と経路

```
  native (Rust) ── harmion-update ── nix build github:.../<tag>#default
                                      └─ ~/.local/share/harmion/profiles/<tag>  (GC root)
                                      └─ ~/.local/bin/harmiond など（symlink）

  Android app ───── GitHub Release node-vX.Y.Z / mote-vX.Y.Z ──► Obtainium

  Worker ────────── GitHub Actions ──► wrangler deploy ──► Cloudflare（prod）
```

| 成果物 | トリガ | 中身 |
|---|---|---|
| native | `vX.Y.Z` タグ | GitHub Release のバイナリは作らない。`nix build` の profile を `~/.local/bin` から参照する |
| `harmion-node-vX.Y.Z.apk` | `node-vX.Y.Z` タグ | 署名済み APK、`mapping.txt`、`SHA256SUMS`、`cert-sha256.txt` |
| `mote-vX.Y.Z.apk` | `mote-vX.Y.Z` タグ | 同上 |
| Worker | `vX.Y.Z` タグ or `workflow_dispatch` | wasm + admin UI を `wrangler deploy` |

最初のタグは `v0.1.0` / `node-v0.1.0` / `mote-v0.1.0` でよい。RC は `-rc.1` を付けて prerelease にする。

## 2. リポジトリ構造の見直し

### 2.1 結論: 分割しない

今の構造（1 つの Cargo workspace + `android/` の Gradle multi-project + `deploy/cloudflare`）は、このシステムの結合度に対して妥当です。

- Android SDK はノードアプリの署名証明書をピンするので、app と SDK と Rust core は同じリポジトリで一緒に進めた方がよい。
- Worker は `crates/worker` 経由で `node` / `protocol` / `domain` を共有する。
- native の daemon / CLI / admin は同じ `node` / `runtime` / `store-sqlite` を使う。
- リリースの互換性（protocol v1、SDK のピン、Worker の schema）を 1 つのタグで説明できる。

分割すると「どの組み合わせが動くか」をリポジトリ間で管理することになり、今の規模では損です。Android SDK を外部配布する段階になっても、まず AAR を Release に添付するだけで足ります。

### 2.2 先にほどく 3 つの結合

#### (1) Cargo ビルドが npm を要求する

`apps/admin/build.rs` が `apps/admin/ui/node_modules` を assert し、`npm run build` を実行しています。そのため `cargo test --workspace` にも npm が必要で、Nix derivation も複雑になります。

**直し方**

- `apps/admin/ui` を独立した Nix package（`buildNpmPackage`）にする。
- `build.rs` は `HARMION_ADMIN_UI_DIST` があればその directory を埋め込み、なければ従来どおり npm を呼ぶ（開発時のフォールバック）。
- Nix / CI は `HARMION_ADMIN_UI_DIST=$(nix build .#admin-ui --print-out-paths)` を渡す。
- これで `harmiond` / `harmion` のビルドは npm と無関係になり、`harmion-admin` だけが UI に依存する。

#### (2) Android の JNI ビルドが Gradle の依存に入っていない

`jniLibs/` は gitignore され、`android/scripts/build-rust.sh` を手で呼ぶ前提です。`release-1password.sh` は呼んでいないため、clean checkout からの release は native lib なしで APK を作れてしまいます。

**直し方**

- `android/build.gradle.kts` に `buildRust` タスクを追加し、各 app の `preBuild` から依存させる。
- あるいは `release-app.sh` が必ず `build-rust.sh` を先に呼ぶ。CI は Gradle だけを呼ぶ形にする。
- `assembleRelease` の前に `.so` の存在と ABI を検査する。

#### (3) バージョンとリリース対象が散っている

- crate の version が各 `Cargo.toml` に `0.1.0` で散在。
- Android の `versionCode` が app は Gradle property、mote はハードコード。
- どのアプリを出すかの一覧がない。

**直し方**

- `[workspace.package] version` を追加し、各 crate は `version.workspace = true`。
- `android/<app>/version.properties` を正にする。
- `android/release-apps.toml` を追加する。

### 2.3 追加するファイル

```
.github/workflows/
  ci.yml
  release-android.yml
  deploy-worker.yml
android/
  release-apps.toml
  app/version.properties
  mote/version.properties
  scripts/release-app.sh        # release-1password.sh を一般化
scripts/
  release.sh                   # 版 bump + tag
  harmion-update               # nix build → ~/.local/bin（インストール/更新）
flake.nix                      # packages を追加
```

### 2.4 今はやらないこと

- リポジトリの分割（native / android / worker）
- `deploy/cloudflare` の `apps/` への移動（実体はアプリだが、CI 開始のブロッカーではない）
- `design_checks` の `tests/` への移動
- `crates/android` と `crates/client-ffi` の `crates/jni/` への統合
- Cargo の `members` を glob 化する（意図しない crate を拾う）
- `rust-toolchain.toml` の追加（Nix が正。二重管理になる）
- SDK の AAR 配布（外部利用者が出るまで不要）

これらは CI が回ってから、必要性が出たときにやる。今やると差分が大きくなり、CI のデバッグと構造変更が混ざる。

## 3. Phase 0: 土台

### 3.1 作業

- [ ] `Cargo.toml` に `[workspace.package] version = "0.1.0"` を追加し、各 crate を `version.workspace = true` に
- [ ] `android/app/version.properties` と `android/mote/version.properties` を追加
  ```properties
  versionCode=1
  versionName=0.1.0
  ```
- [ ] `android/release-apps.toml` を追加
  ```toml
  [node]
  op_item = "Harmion release keystore"

  [[app]]
  module = "app"
  application_id = "dev.satler.harmion"
  tag_prefix = "node-v"
  version_file = "android/app/version.properties"
  op_item = "Harmion release keystore"
  ship = true

  [[app]]
  module = "mote"
  application_id = "dev.satler.mote"
  tag_prefix = "mote-v"
  version_file = "android/mote/version.properties"
  op_item = "Mote release keystore"
  ship = true

  [[app]]
  module = "testclient"
  ship = false
  ```
- [ ] Gradle から `version.properties` を読む（全 app 共通の小さな convention か、各 `build.gradle.kts` の数行）
- [ ] `buildRust` を Gradle の `preBuild` に接続
- [ ] `apps/admin/build.rs` に `HARMION_ADMIN_UI_DIST` を追加
- [ ] `apps/admin/ui` を `buildNpmPackage` で包む（Phase 1 で）
- [ ] `.deploy-secrets/` と `harmion-admin-root.pem` を 1Password の専用 vault へ移す
- [ ] ルートの `admin-ui/` 残骸を削除
- [ ] GitHub の secret scanning / push protection を有効化

### 3.2 完了条件

- `cargo build --release -p harmiond -p harmion -p harmion-admin` が、`HARMION_ADMIN_UI_DIST` を渡した状態で通る
- `cargo build --release -p harmiond -p harmion` が npm なしで通る
- `gradle :app:assembleRelease` が clean checkout で `.so` を生成する
- `versionCode` が 1 か所で決まる

## 4. Phase 1: Nix からビルドできるようにする

### 4.1 flake に追加する package

```nix
packages.<system> = {
  admin-ui = ...;          # buildNpmPackage
  harmiond = ...;          # dynamic（NixOS 用）
  harmion = ...;
  harmion-admin = ...;     # admin-ui に依存
  default = ...;           # harmiond + harmion + harmion-admin の buildEnv
};
```

- dynamic は `rustPlatform.buildRustPackage`（fenix stable + `makeRustPlatform`）で、`cargoLock.lockFile = ./Cargo.lock`。
- `src = self` で flake の git-tracked source を使う。`target/` は入らない。
- `harmion-admin` は `HARMION_ADMIN_UI_DIST = "${admin-ui}"` を設定。
- `default` は `buildEnv` で 3 つのバイナリと systemd unit をまとめる。`harmion-update` はこれを 1 回の `nix build` で取る。
- `wiki-gui` は `anti-webui` の git 依存があるので、最初は `default` に入れない。必要なら個別 package にする。
- `nix build .#harmiond` / `.#harmion` / `.#harmion-admin` / `.#default` を CI と手元の両方で実行できるようにする。

### 4.2 なぜ musl は不要か

最初の案では GitHub Release に静的バイナリを置くために musl を使うつもりだった。しかし今回のインストールは「このマシンで Nix ビルドしたものを `~/.local/bin` に置く」なので、musl は不要。

今 `~/.local/bin/harmiond` に入っているバイナリも、実は Nix の glibc を指している。

```
interpreter: /nix/store/r3h...-glibc-2.44-57/lib/ld-linux-x86-64.so.2
RUNPATH:     /nix/store/r3h...-glibc-2.44-57/lib:...
```

これが動いているのは、その store path がまだ存在するから。`nix-collect-garbage` で消えると動かなくなる。だから **静的化するのではなく、profile の out-link を GC root にして store path を保持する**。

- `nix build --out-link` の後、`nix-store --add-root ~/.local/share/harmion/profiles/<tag> --indirect -r <store-path>` で profile を GC root として登録する（out-link を mv すると auto root が切れるため）。
- その profile の `bin/` を `~/.local/bin` から symlink する。
- 旧 profile を残せば rollback もできる。
- マシン移動や Nix なし環境への配布が必要になったら、そのときだけ `harmion-static`（musl）を追加する。今はやらない。

### 4.3 `admin-ui` package

```nix
admin-ui = pkgs.buildNpmPackage {
  pname = "harmion-admin-ui";
  version = "0.1.0";
  src = ./apps/admin/ui;
  npmDepsHash = "sha256-...";   # package-lock.json から
  npmBuildScript = "build";
  installPhase = "mkdir -p $out && cp -r dist/. $out/";
};
```

`npmDepsHash` は `nix build` のエラーに出る値を採用する。`package-lock.json` を更新したときだけ変わる。

### 4.4 完了条件

- `nix build .#harmiond .#harmion .#harmion-admin .#default` が通る
- `nix develop` の開発フローは今までどおり
- `nix flake check` が評価できる
- 手で `nix build .#default --out-link /tmp/harmion-profile` して `bin/harmiond --version` が通る

補足: Nix package では `doCheck = false` にして、テストは CI の `cargo test` に任せる。Nix sandbox 内でネットワークや localhost に依存するテストを走らせないため。

## 5. Phase 2: PR CI

### 5.1 workflow

`.github/workflows/ci.yml`。PR と main で実行。

| job | 内容 |
|---|---|
| `changes` | `dorny/paths-filter` で rust / android / worker / docs を判定 |
| `rust` | `cargo fmt`、`clippy`、`test --workspace`。`HARMION_ADMIN_UI_DIST` は `nix build .#admin-ui` で用意 |
| `nix` | `nix build .#harmiond .#harmion .#harmion-admin`（PR では static を除いて時短） |
| `android` | `nix develop .#android` で `gradle :app:assembleDebug :sdk:assembleDebug :mote:assembleDebug`。`buildRust` が動くことも確認 |
| `worker` | `cargo fetch` + `deploy/cloudflare/build.sh` |
| `lint` | `actionlint`、`gitleaks`（または `osv-scanner`） |

### 5.2 セキュリティ

- `permissions: contents: read` をデフォルトに。
- `pull_request_target` は使わない。
- action は SHA で pin。Dependabot / Renovate で更新。
- PR では 1Password も Cloudflare も触らない。
- Cachix は read のみ。push は main / tag だけ。

### 5.3 キャッシュ

- Nix: `nix-community/cache-nix-action`（runner の store）
- Cargo: `Swatinem/rust-cache`（`Cargo.lock`、target triple ごと）
- Gradle: `gradle/actions/setup-gradle`
- npm: `actions/setup-node` の cache

初回は devShell の closure 取得が重い。main で Cachix に push しておくと 2 回目以降が速い。

### 5.4 完了条件

- テスト用 PR で全 job が緑
- fork からの PR に secret が渡らない
- docs だけの PR で Android / Worker が走らない（path filter）

## 6. Phase 3: native のインストールと更新

### 6.1 `harmion-update` の設計

native は GitHub Release のバイナリを作らない。**タグ + flake + `nix build`** で入れる。dotfiles も home-manager も介さない。

**配置**

```
~/.local/share/harmion/profiles/v0.1.0   -> /nix/store/...-harmion-0.1.0   (GC root)
~/.local/share/harmion/current           -> profiles/v0.1.0
~/.local/share/harmion/previous          -> profiles/v0.0.9
~/.local/bin/harmiond                    -> ../share/harmion/current/bin/harmiond
~/.local/bin/harmion                     -> ../share/harmion/current/bin/harmion
~/.local/bin/harmion-admin               -> ../share/harmion/current/bin/harmion-admin
~/.local/bin/harmion-update              -> ../share/harmion/current/bin/harmion-update
```

- `profiles/<tag>` は `nix build --out-link` の後、`nix-store --add-root` で GC root として登録する。out-link をそのまま `mv` すると Nix の auto root が切れるので、移動後に登録し直す。
- `current` は「今使う profile」を指すだけ。`profiles/<tag>` を消さない限り store path は GC されない。
- 旧 profile を残すので、`--rollback` で `current` を前の profile に戻せる。
- `~/.local/bin` にあるのは symlink だが、PATH からは普通のコマンドとして使える。

**処理の流れ**

1. tag を解決（引数、または最新の stable tag）
2. `nix build "github:satler-git/harmion/$tag#default" --out-link "$HOME/.local/share/harmion/profiles/.tmp-$tag"` でビルド
3. `nix-store --add-root "$HOME/.local/share/harmion/profiles/$tag" --indirect -r "$(readlink -f ...)"` で profile を GC root にする
4. `current` を新しい profile に原子的に付け替える（`ln -sfn` + `mv`）
5. `~/.local/bin` の symlink を作り直す
6. `harmiond --version` で確認
7. `--systemd` なら `harmion@.service` を `~/.config/systemd/user/` に置いて `daemon-reload`。`--restart` のときだけサービスを再起動

**オプション**

```
harmion-update [--version vX.Y.Z] [--latest] [--rollback]
               [--list] [--prune] [--dry-run]
               [--systemd] [--restart] [--from-local PATH]
```

- 引数なし: 最新の stable tag を GitHub の tags から解決（`v*-rc.*` は除外）。
- `--version vX.Y.Z-rc.1`: RC を試す。
- `--rollback`: 1 つ前の profile に戻す。
- `--list`: `~/.local/share/harmion/profiles/` の一覧。
- `--prune`: 古い profile の out-link を消す（消した store path は GC 対象になるので、実行前に確認）。
- `--from-local PATH`: ネットワークを使わず、手元の checkout を `nix build` する。
- `--dry-run`: 何をビルドし、どこに symlink するかだけ表示。

**初回の移行**

今の `~/.local/bin/harmiond` などは実ファイル。初回は次を行う。

- 既存ファイルを `~/.local/share/harmion/pre-install/` に退避する。
- 退避したことを表示してから symlink に置き換える。
- 既存の systemd unit はそのまま（`ExecStart=%h/.local/bin/harmiond` は symlink をたどるので変更不要）。

**ビルドを速くする**

- `nix build github:...#default` は初回に Rust の依存をビルドする。CI がタグ時に `.#default` を Cachix（`satler.cachix.org`）へ push しておけば、更新はほぼ download で済む。
- Cachix が落ちていてもローカルビルドで動く。必須依存にはしない。

### 6.2 CI 側

native 用の GitHub Release は不要。タグ時に次だけ行う。

- `nix build .#default` が通ることを確認
- Cachix へ push（任意だが強く推奨）
- `nix flake check`

`release-native.yml` のような Release 作成 workflow は作らない。Android と Worker だけが Release / deploy を持つ。

### 6.3 daemon の更新手順

`harmion-update` はバイナリを置くだけで、サービスは触らない（`--restart` を除く）。実際の更新は次の順。

```sh
systemctl --user stop harmion@main.service
# ノードディレクトリをバックアップ（linux-service.md）
harmion-update --version v0.1.0
systemctl --user start harmion@main.service
```

- 旧 profile は `~/.local/share/harmion/profiles/` に残る。起動に失敗したら `harmion-update --rollback`。
- schema migration が走る可能性があるので、旧バイナリで新 DB を読めるとは限らない。バックアップを必ず取る。

### 6.4 完了条件

- `v0.1.0` タグから `nix build github:satler-git/harmion/v0.1.0#default` が通る
- `harmion-update --version v0.1.0` で `~/.local/bin/harmiond` などが symlink になり、`harmiond --version` が通る
- `nix-store -q --roots ~/.local/share/harmion/profiles/v0.1.0` で GC root を確認できる
- `harmion-update --rollback` で前の profile に戻る
- daemon を停止 → 更新 → 起動して node が読める
- `~/.local/bin` が home-manager 管理でないことを確認済み

## 7. Phase 4: Android の release と Obtainium

### 7.1 `release-app.sh`

`release-1password.sh` を一般化し、`android/release-apps.toml` を読む。

1. タグから app を決める（`node-v*` → `app`、`mote-v*` → `mote`）
2. `version.properties` とタグの `versionName` を照合
3. 前回タグの `versionCode` より大きいことを確認
4. 1Password から keystore とパスワードを取得（`op_item` を使用）
5. Gradle の `buildRust` タスクが `arm64-v8a` をビルド（`-PharmionRustAbis` で指定）
6. `gradle :<module>:assembleRelease` に証明書ピンを渡す（version は `version.properties` から読む）
7. `apksigner verify --print-certs` で検証
8. `target/release/<artifact>-vX.Y.Z.apk`、`mapping.txt`、`cert-sha256.txt`、`SHA256SUMS` を出力

### 7.2 workflow

`.github/workflows/release-android.yml`:

- トリガ: `node-v*`、`mote-v*`
- `environment: release-android`（required reviewers）
- `OP_SERVICE_ACCOUNT_TOKEN` を Environment secret から
- `nix develop .#android --command android/scripts/release-app.sh "$TAG"`
- `gh release create`（prerelease 対応）

### 7.3 `versionCode` のルール

- アプリごとに独立。
- 前回の `<app>-v*` タグの `version.properties` と比較して、必ず +1 以上。
- 同じ `versionCode` の Release が既にあれば失敗。
- RC も `versionCode` を消費する（Play は使わないが、Obtainium の更新判定に使われる）。RC の後で正式版を出すときはさらに +1 する。

### 7.4 Obtainium

- アプリごとに登録する。README の「Add to Obtainium」ボタンは、この設定を入れた full config deep link。
  - node: repo `satler-git/harmion`、パッケージ名 `dev.satler.harmion`
  - mote: 同じ repo、`dev.satler.mote`
- Obtainium の filter は APK ファイル名への正規表現で、パッケージ名では絞れない。同じ repo に2アプリあるため、release title と APK filename の両方で分ける。
  - node: title `^node-v`、APK `^harmion-node-`
  - mote: title `^mote-v`、APK `^mote-`
- tag が `node-v0.1.0` のままなので、version trim を `v([0-9]+\.[0-9]+\.[0-9]+)`、match group `1` にして Android の `versionName` と比較する。
- prerelease は含めない（RC を試すときだけ一時的に有効化）。
- 更新判定は versionName（必要なら versionCode）で行う。タグの semver と `versionCode` の両方を上げる。

### 7.5 完了条件

- `node-v0.1.0` で署名済み APK が Release に出る
- Obtainium でインストール → `node-v0.1.1` で更新でき、データが残る
- SDK の証明書ピンが release 証明書と一致する（debug 証明書に落ちていない）
- `mote-v0.1.0` も同様

## 8. Phase 5: Worker deploy

### 8.1 workflow

`.github/workflows/deploy-worker.yml`:

- トリガ: `v*` タグ（native と同じ）と `workflow_dispatch`
- `environment: cloudflare-prod`（required reviewers）
- `nix develop --command` で `cargo fetch` + `deploy/cloudflare/build.sh`
- `wrangler deploy --cwd deploy/cloudflare`
- smoke: `curl -fsS https://hub.satler.dev/state`、`/ready`
- 失敗時は deploy を止める（自動 rollback はしない）

### 8.2 secret

- `CLOUDFLARE_API_TOKEN` は GitHub Environment `cloudflare-prod` の secret。scope は Workers Scripts:Edit + 対象 zone の Workers Routes:Edit。
- `SEED` / `ADMIN_TOKEN` / `BOOTSTRAP_TOKEN` は **CI に渡さない**。owner が 1Password から `wrangler secret put` する。
- `wrangler.toml` の `NODE_ID` / `EPOCH` / `CLUSTER` / `ORIGIN` は公開情報としてレビューする。deploy で変えない。

### 8.3 staging の方針

初期は作らない。理由:

- 個人用の replica で、外部利用者がいない。
- DO の SQL は `design_checks/worker_do` の workerd probe でローカル検証できる。
- `wrangler` の前 version 保持と `wrangler rollback` でコードは戻せる。

ただし **DO の migration を追加する PR では、一時的に `harmion-hub-staging` を立てる**。staging は別 `NODE_ID` / `SEED` / domain / DO にする。migration が不要なら作りっぱなしにしない。

### 8.4 完了条件

- `workflow_dispatch` で prod に deploy でき、`/state` と `/ready` が 200
- 前 version に rollback できることを一度確認
- `SEED` が deploy で変わっていない（`provision-status` か `/version` で確認）

## 9. Phase 6: 仕上げ（任意）

- Renovate / Dependabot で actions・Cargo・npm・Gradle・flake.lock を更新
- `cargo audit` / `osv-scanner` / `npm audit` を nightly
- emulator E2E（`owner-smoke.sh`、承認フロー）を nightly
- `wiki-gui` / `wiki-server` / `wiki-lsp` も Nix package 化（今は native の 3 つを優先）
- Cachix に `.#default` を push（`harmion-update` のビルドを速くする）
- 非 Nix 環境向けに static バイナリ（musl）の Release が必要になったら追加する
- arm64 native（必要になったら）
- SDK AAR の配布（外部アプリが出たら）
- Worker の staging 常設（migration が頻繁になったら）

## 10. 順番と目安

| Phase | 内容 | 目安 | 依存 |
|---|---|---|---|
| 0 | 土台（version、manifest、JNI、admin UI 分離） | 0.5〜1 日 | — |
| 1 | Nix packages（dynamic + `default`） | 1〜2 日 | Phase 0 |
| 2 | PR CI | 0.5〜1 日 | Phase 1 |
| 3 | native の `harmion-update` + Cachix | 1 日 | Phase 1 |
| 4 | Android release + Obtainium | 1 日 | Phase 0 |
| 5 | Worker deploy | 0.5 日 | Phase 2 |

Phase 3 と 4 は Phase 1 の後に並行できる。最初の 1 週間で「`harmion-update` で `~/.local/bin` に native を入れられる」ところまで行くのが最短。

## 11. リスク

| リスク | 影響 | 対策 |
|---|---|---|
| Nix store の GC でバイナリが壊れる | `~/.local/bin` の symlink 先が消える | `profiles/<tag>` を `nix-store --add-root` で GC root にし、`current` はその上に張る。`--prune` は明示時だけ |
| `admin-ui` の `npmDepsHash` が面倒 | Phase 1 が伸びる | 最初は `harmiond` / `harmion` だけ package 化し、admin は後回しにしてもよい。ただし `default` には admin を含めたいので Phase 3 までに解消 |
| Android の署名鍵・versionCode 事故 | 更新不能・ストア不整合 | 鍵は 1Password + オフライン backup。versionCode はタグとファイルの二重検査。RC でリハーサル |
| GitHub runner の Nix 初回が遅い | CI が 30 分超え | cache-nix-action + Cachix。PR では `default` をビルドしない |
| Worker の identity を deploy で壊す | node が別 identity になる | CI は secret を触らない。`wrangler.toml` の変更をレビュー。deploy 後 smoke |
| `harmion-update` の配置ミス | 手元の更新が止まる／壊れた symlink が残る | profile の atomic 切替、`--rollback`、`--dry-run`、既存バイナリの退避 |

## 12. 確認したいこと

1. native は「タグを `nix build` して `~/.local/bin` に symlink」でよいか（GitHub Release のバイナリは作らない）。
2. `~/.local/bin` は実ファイル置き場か、`profiles/<tag>` + symlink（推奨）でよいか。
3. 最初のタグは `v0.1.0` でよいか（CI 整備後の最初のリリースを `v0.2.0` にするか）。
4. Cachix（`satler.cachix.org`）へ `.#default` を push してよいか。
5. 1Password の専用 vault 名は `harmion`、Worker の prod domain は `hub.satler.dev` のままでよいか。
