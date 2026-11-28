# Harmion の出荷（CI/CD）設計

状態: 設計 / 対象: このリポジトリ全体 / 最終更新: 2026-11-28

この文書は「どう作るか」ではなく「どう署名し、どう配り、secret をどこに置くか」の設計です。
実装状況は [ci-plan.md](ci-plan.md) を参照する。Android の署名リリース・native の Nix package・CI の段構成は実装済み。

> **2026-11-26 の決定**: Android は GitHub Releases + Obtainium。native は `harmion-update` が GitHub のタグから `nix build` し、`~/.local/bin` に symlink する。dotfiles / home-manager は介さない。musl static は作らない（非 Nix 環境向けに必要になったら追加）。Worker の staging は初期は常設しない。実行プランは [ci-plan.md](ci-plan.md) を優先する。以下のうち「dotfiles が消費する」「staging を常設する」を前提にした記述は、この決定で置き換える。

## 0. 結論（要約）

1. **GitHub Actions 一本**にする。public リポジトリなので minutes は無料で、Release・attestation・Environment 保護がそのまま使える。
2. **ツールチェーンの正は Nix devShell**。CI も手元も `nix develop` 経由で同じ rustc / JDK / NDK / Gradle / wasm-bindgen / wrangler を使う。
3. **配布の契約は Nix の `packages`**。native は `nix build` の成果物を `harmion-update` で `~/.local/bin` に入れる。dotfiles は介さない。
4. **タグは「レーン」単位**。Rust 全体 + Worker は `vX.Y.Z`、Android アプリは `<app>-vX.Y.Z`。`versionCode` はアプリごとに単調増加を CI が検査する。
5. **secret の正は 1Password**。GitHub には「1Password を読むための token」と「デプロイ用の短命 token」だけを置く。Worker の実行時 secret（`SEED` 等）は CI が書き換えない。
6. **Worker は初期は prod のみ**。Durable Object の schema は前方互換で戻せないため、prod は Environment の承認つきにし、smoke と前 version 保持で戻せるようにする。staging は migration を入れる回だけ一時構築する。
7. **インストールは dotfiles を介さない**。`harmion-update` がタグを `nix build` し、`~/.local/share/harmion/profiles/<tag>` を GC root にして `~/.local/bin` から symlink する。`--rollback` で前の profile に戻せる。

## 1. 現在地

| 対象 | 今の作り | 出荷方法 | 状態 |
|---|---|---|---|
| native（`harmiond` / `harmion` / `harmion-admin` / `mote` / `mote-hud`） | Cargo workspace | flake の `packages` と `harmion-update` | 実装済み |
| Android node app | Gradle + JNI cdylib | [release-apps.toml](../../android/release-apps.toml) → タグで Release | 実装済み |
| Android mote | Gradle app | 同じ。版は `version.properties` が正 | 実装済み |
| Android SDK | Gradle library | リポジトリ内の `project(":sdk")` | AAR 配布なし |
| Worker | `wrangler deploy` | [deploy-worker.yml](../../.github/workflows/deploy-worker.yml)（build → deploy → smoke） | 実装済み |
| Nix | devShells + `packages` | `nix build` / flake | 実装済み |
| CI | GitHub Actions | ci / cache-native / release-android / deploy-worker / third-party-notices | 実装済み |
| 秘密 | 1Password（keystore）+ リポジトリ直下の平文 | CI から `OP_SERVICE_ACCOUNT_TOKEN` で参照 | 平文が残る |

補足:

- `harmion-admin-root.pem` と `.deploy-secrets/` は [`.gitignore`](../../.gitignore) 対象で、git 履歴にも入っていない。ただし平文でリポジトリ直下に置く必要はない。
- `apps/admin/build.rs` は `apps/admin/ui/node_modules` の存在を assert し、`npm run build` を呼ぶ。つまり **`cargo test --workspace` にも npm が要る**。
- `deploy/cloudflare/build.sh` は `cargo build ... --offline` を使う。CI では事前に `cargo fetch` が必要。
- `wiki-gui` はビルド時に `WIKI_SANS_FONT` / `WIKI_MONO_FONT` を `include_bytes!` する。Nix package ではこの env を必ず設定する（未設定だと空フォントで出荷される）。
- Android の JNI ライブラリは [android/build.gradle.kts](../../android/build.gradle.kts) の `buildRust` が全モジュールの `preBuild` から走る。手順としての欠落はない。
- `android/app` と `android/mote` は applicationId が別（`dev.satler.harmion` / `dev.satler.mote`）。署名鍵も分けられる。
- `android/testclient` は `org.harmion.testclient` でデバッグ専用。配布対象にしない。

## 2. 設計の柱

### 2.1 ツールチェーンは Nix、配布は Release

- CI の各 job は `nix develop .#...` の中でコマンドを実行する。`rustup` や `setup-java` を CI 側で個別に固定しない。
- native は `nix build .#harmiond` のように **純粋な derivation** として作る。再現性があり、Cachix に載り、Nix から直接ビルドできる。インストールは `harmion-update` が `~/.local/bin` に symlink する（後述）。
- Android と Worker は devShell 内の Gradle / npm / wrangler を使う。**署名 APK は Nix derivation にしない**（keystore を store に入れないため）。
- CI でだけ差が出る要素（GitHub の runner イメージ、npm の取得、Gradle のキャッシュ）は devShell + lockfile + キャッシュで吸収する。

### 2.2 レーンごとに独立して出せる

アプリが増えても script を増やさないために、**「出荷対象の一覧」をデータにする**。Android は `android/release-apps.toml` のような manifest を置き、workflow はそれを読んで matrix を組む。native / Worker / SDK も同じ考えで、タグの prefix でレーンを決める。

### 2.3 秘密は「実行時に読む」

- keystore、runtime secret、runtime の管理者 token は **リポジトリにも Nix store にも GitHub の secret にも置かない**。CI の実行中だけ 1Password から取得し、job 終了時に消す。
- GitHub に置くのは「1Password を読む権限」と「Cloudflare に deploy する権限」だけ。どちらも Environment secret にして、PR からは触れない。

## 3. 出荷単位とバージョン

### 3.1 レーン

| レーン | 対象 | タグ | 版の正 | 配布先 |
|---|---|---|---|---|
| native | `harmiond`, `harmion`, `harmion-admin`, `wiki-gui`, `wiki-lsp`, `wiki-server`, systemd unit, backup script | `vX.Y.Z` | `[workspace.package].version` | `nix build .#default` + `harmion-update`（`~/.local/bin` symlink） |
| worker | `harmion-worker` wasm + `deploy/cloudflare` + admin UI | `vX.Y.Z`（native と同じタグ。deploy は承認制） | 同上（workspace version） | Cloudflare（初期は prod のみ） |
| Android node | `android/app` | `node-vX.Y.Z` | `android/app/version.properties` | GitHub Release APK（Obtainium） |
| Android mote | `android/mote` | `mote-vX.Y.Z` | `android/mote/version.properties` | GitHub Release APK（Obtainium） |
| Android SDK | `android/sdk` | `sdk-vX.Y.Z`（配布する場合） | `android/sdk/version.properties` | まず AAR を Release 添付。外部配布するなら GitHub Packages |
| testclient | `android/testclient` | なし | — | 配布しない（debug のみ） |

Rust の native と Worker は同じ workspace version に乗せる。Worker は「タグが打たれたら必ず deploy」ではなく、**同じタグから prod 承認つきで deploy できる**形にする。これで「native だけ出したい」「Worker も一緒に出す」を運用で選べる。

アプリが増えたときの追加は次の 4 点だけにする。

1. `android/<app>/version.properties` を置く
2. `android/release-apps.toml` に 1 エントリ足す
3. 1Password に署名アイテムを作る（アプリごとに別鍵）
4. applicationId と SDK のピン（ノードアプリの証明書）を確認する

### 3.2 `versionCode`

`versionCode` は **アプリごとの `version.properties` を正**にする。式で自動計算しない（RC や再アップロードで破綻しやすい）。

```properties
# android/mote/version.properties
versionCode=2
versionName=0.2.0
```

CI は次を検査する。

- 前回の `<app>-v*` タグの `version.properties` より `versionCode` が大きい
- タグの `versionName` とファイルの `versionName` が一致する
- 同じ `versionCode` の Release が既に存在しない

`assembleRelease` へは Gradle property で渡す（現在の `-PharmionVersionCode` / `-PharmionVersionName` の仕組みを全アプリで統一する）。

### 3.3 リリースノート

- `git-cliff` などで conventional commit から生成する。レーンごとに path で filter する（`android/mote/**` など）。
- 必ず次を含める。
  - 版、タグ、commit SHA
  - 成果物と SHA-256
  - Android: `versionCode`、署名証明書の SHA-256
  - 互換性: contract / protocol の版（`contracts/*-v1.md`）、必要なら Worker の版
- RC は `vX.Y.Z-rc.N` / `<app>-vX.Y.Z-rc.N`。`harmion-update --version vX.Y.Z-rc.N` で先に試せる。

## 4. ブランチとリリースフロー

```
PR ──► main（常に green、trunk-based）
        │
        ├─ release PR（版 bump + changelog）──► merge
        │                                      │
        │                                      └─ tag ──► lane workflow ──► artifact / deploy
        │
        └─ hotfix は tag から branch → patch を main に戻す
```

- `main` への merge は PR + 必須 CI。直 push はしない（自分の repo でも ruleset で縛る）。
- タグは **main の HEAD に打つ**。release PR を merge したら、その commit に注釈付きタグを打って push する。
- タグ保護（ruleset）で `v*`・`*-v*` の作成・削除を owner だけに制限する。
- `workflow_dispatch` からの再実行を必ず用意する（同じタグで rebuild はできるが、成果物の上書きはしない。Release は immutability を有効にする）。
- hotfix は「tag から切った branch で直して patch タグ」を基本にし、同じ修正を main へ cherry-pick する。main だけ直して古い版の利用者を置き去りにしない。

### 4.1 版 bump の自動化

最初は小さな `scripts/release.sh` でよい。

```sh
# 例: native を 0.7.0 にする
scripts/release.sh native 0.7.0
# 例: mote を 0.2.1 にする
scripts/release.sh android mote 0.2.1
```

やること:

1. `main` が clean で origin と一致することを確認
2. 対象の version（`Cargo.toml` の workspace version / `version.properties`）を更新
3. changelog を生成
4. `chore(release): ...` で commit
5. 注釈付きタグを作成して push（CI が release を実行）

`release-please` や `cargo-release` は、レーンが 3 つ以上になって手作業がつらくなったら導入する。最初から入れると Gradle の version file やタグ prefix の設定でハマる。

## 5. CI の段構成

| 段 | トリガ | 目的 | 目安 |
|---|---|---|---|
| PR | `pull_request` | 壊れたものを merge させない。署名・deploy はしない | 10〜20 分 |
| main | `push: main` | 統合後のフル検証。Cachix へ push。Worker は build のみ | 20〜40 分 |
| release | `push: tag` | 成果物の作成・署名・Release・prod deploy | 20〜40 分 |
| nightly | `schedule` | emulator E2E、複数プロセス同期、Worker probe、依存監査 | 1 時間以内 |

### 5.1 PR

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace --locked`（`npm ci --prefix apps/admin/ui` の後）
- `npm ci` + `npm run build`（admin UI 単体でも確認）
- `cargo build -p harmion-worker --target wasm32-unknown-unknown --locked` + wasm-bindgen
- Android: `gradle :app:assembleDebug :sdk:assembleDebug :mote:assembleDebug`（JNI は 1 ABI だけビルドして link を確認）
- `nix flake check`（評価。重ければ PR では `nix flake show --all-systems` に留める）
- secret scan（GitHub の secret scanning + push protection、`gitleaks` か `osv-scanner`）
- workflow の lint（`actionlint`、`zizmor`）

PR では Cachix への **push をしない**（fork PR に token を渡さない）。読み取りは public cache から行う。

### 5.2 main

- PR の全項目
- `nix build` で `packages` を全部ビルドし、Cachix へ push
- `design_checks/model_check.py`、`two_process_sync.py`、`multi_process_sync.py`、`membership_process_sync.py`（`cargo build --workspace` 後）
- Worker の probe（`design_checks/worker_sql`、`worker_do`）
- Worker の build を検証（deploy はタグ時 / 手動）
- Android の unit test（`:app:testDebugUnitTest` など）

### 5.3 release（タグ）

- タグの prefix でレーンを判定する。
- native: `nix build` → tarball 化 → SHA-256 → `gh release create` → attestation
- Android: manifest の matrix → JNI ビルド → Gradle → 署名 → `apksigner verify` → Release
- Worker: build → prod 承認 → deploy → smoke → 必要なら前 version へ rollback
- 同じタグの再実行は「成果物の再作成」まで。Release asset の上書きはしない。

### 5.4 nightly / weekly

- emulator での E2E（`owner-smoke.sh`、承認フロー）
- 5 プロセス同期・membership の実プロセス試験
- Worker の workerd probe
- `osv-scanner` / `cargo audit` / `npm audit`
- flake.lock の更新 PR（Renovate か `update-flake-lock`）
- Dependabot / Renovate で actions・Cargo・npm・Gradle の更新 PR

### 5.5 キャッシュ

- Nix: `nix-community/cache-nix-action`（ephemeral runner の store キャッシュ）+ Cachix（cross-run / 再ビルド用）
- Cargo: `Swatinem/rust-cache`（`Cargo.lock` と target triple で key。Android/wasm の target dir も含める）
- Gradle: `gradle/actions/setup-gradle`。wrapper は置かず Nix の `gradle` を使うので、キャッシュだけ使う
- npm: `actions/setup-node` の cache（`apps/admin/ui/package-lock.json`）
- 初回は devShell の closure 取得が重い。main で Cachix に devShell の closure も push しておくと 2 回目以降が速い（必須ではない）。

## 6. レーン別の実装形

### 6.1 native（Rust）

**ビルド**

- `nix build .#harmiond` などの純粋ビルドを正とする。
- flake に `packages.<system>` を追加する。中身は `rustPlatform.buildRustPackage`（fenix の stable を `makeRustPlatform` で使う）で、`cargoLock.lockFile = ./Cargo.lock`、`cargoBuildFlags = [ "-p" ... ]`。
- `harmion-admin` は `apps/admin/build.rs` が npm を呼ぶ。Nix では `fetchNpmDeps`（`npmDepsHash` 固定）で `node_modules` を作り、`NPM=${nodejs}/bin/npm` を渡す。ここが最初の関門。難しければ最初は `harmiond` / `harmion` だけ package 化し、`harmion-admin` は devShell ビルドのままにして後から追いついてよい。
- `wiki-gui` は `WIKI_SANS_FONT` / `WIKI_MONO_FONT` をビルド時に設定する。フォントは `include_bytes!` されるので実行時依存にはならない。
- `wiki-gui` の `anti-webui` は git 依存（rev 固定）。`cargoLock` で解決できるが、ビルドに `git` とネットワークが要る。Nix の sandbox で問題になるなら `fetchgit` を明示する。
- version は `[workspace.package] version` に集約し、各 crate は `version.workspace = true` にする（現状は各 crate に `0.1.0` が散っている）。`--version` の出力がタグと一致することを release CI で確認する。

**成果物**

- `harmion` / `harmiond` / `harmion-admin` / `wiki-gui` / `wiki-lsp` / `wiki-server` のバイナリ
- `deploy/systemd/*.service`、`deploy/backup/harmion-backup.sh`
- これらを 1 つの `harmion` package（`bin/` + `share/harmion/`）にまとめると取り回しが楽。個別 package も残す。

**配布**

- `nix build .#default` の成果物を `harmion-update` が `~/.local/bin` に symlink する。GitHub Release の native バイナリは作らない。
- `nix build .#harmiond` / `.#harmion` / `.#harmion-admin` を個別にも使える。
- Release 用の静的バイナリが必要になったら、そのときだけ `harmion-static`（musl）を追加する。今は作らない。
- native のタグは GitHub のタグ（`vX.Y.Z`）と Cachix のビルドキャッシュで表現する。attestation は Android の APK に付ける。

**systemd**

- unit は `default` package の `share/systemd/user/` に入れる。`harmion-update --systemd` で `~/.config/systemd/user/` に置き、`--restart` のときだけサービスを再起動する。
- dotfiles / home-manager の module は作らない。

### 6.2 Android

**前提（先に直すこと）**

- **対応済み**: `android/build.gradle.kts` の `buildRust` タスクを各 module の `preBuild` に接続した。clean checkout からでも Gradle が `android/scripts/build-rust.sh` を呼び、`jniLibs/` を生成してから APK を組み立てる。
- `versionCode` / `versionName` をアプリごとの `version.properties` に移す。mote は現在ハードコード。
- `compileSdk 37.2` / build-tools `37.0.0` / NDK `30.0.16248370` はかなり新しい。CI は `nix develop .#android` で同じものを使う。emulator と system image は release job には不要なので、devShell を `android`（ビルド用）と `android-emu`（emulator 用）に分けると CI が軽くなる。

**ビルド（node app の例）**

```sh
nix develop .#android --command bash -euo pipefail -c '
  ./android/scripts/build-rust.sh arm64-v8a x86_64
  cd android
  gradle --no-daemon :app:assembleRelease \
    -PharmionVersionCode="$VERSION_CODE" \
    -PharmionVersionName="$VERSION_NAME" \
    -PharmionNodeCertSha256="$CERT_SHA256"
'
```

- `arm64-v8a` は実機用、`x86_64` は emulator 用。配布 APK を小さくするなら ABI split か AAB を検討する（現状は 1 ABI あたり約 29 MB の `.so`）。
- `-PharmionNodeCertSha256` は **ノードアプリの署名証明書**。SDK を使うアプリ（mote など）にも同じ値を渡す。release では必ず 1Password の keystore から計算した値を使い、SDK の debug 既定値に落とさない。
- `mote` は `project(":sdk")` を使うので、node app と同じ release 証明書ピンでビルドする。

**署名**

- 1Password の service account（read-only）で keystore Document とパスワードを取得する。
- `$RUNNER_TEMP` に `umask 077` で置き、`trap` で必ず消す。ログに値を出さない。
- `keytool` で証明書 SHA-256 を計算し、1Password の `cert_sha256` と照合する（既存 script の検査を維持）。
- `apksigner verify --print-certs` で最終 APK を検証する。
- **アプリごとに別の署名鍵**にする。ノードアプリの鍵は SDK の信頼の起点なので、特に厳重に扱う。applicationId が違えば同じ鍵で複数アプリを署名することも技術的には可能だが、blast radius を考えて分ける。

**配布**

- GitHub Release に APK、`mapping.txt`、`SHA256SUMS`、`cert-sha256.txt`、`version.txt` を添付する。
- クライアント側の更新は Obtainium（GitHub Releases を監視）を第一候補にする。自分用なら F-Droid の self-hosted repo まで作る必要はない。
- **Play を使う場合の注意**: Play App Signing では端末に入る証明書が Google の app signing certificate になる。SDK のピンは upload key ではなく **Play Console の app signing certificate の SHA-256** にする。upload key は 1Password。Play は AAB が必要で、`versionCode` はアップロードごとに単調増加。self-host のうちは APK でよい。
- `testclient` は絶対に Release に載せない（manifest で `ship = false`）。

**SDK（将来）**

- リポジトリ内のアプリは `project(":sdk")` で足りる。外部アプリに配る段階で AAR + POM を GitHub Packages へ publish する。
- SDK の証明書ピンはコンパイル時定数なので、ノードアプリの鍵を変えると SDK の再ビルド・再配布が必要。ノード鍵はアプリの寿命と同じと考える。

### 6.3 Worker（Cloudflare）

**ビルド**

```sh
nix develop --command bash -euo pipefail -c '
  cargo fetch --locked
  deploy/cloudflare/build.sh
'
```

- `build.sh` は `--offline` なので、CI では先に `cargo fetch` する（または CI 用に `--offline` を外す）。
- wasm-bindgen は `crates/worker/Cargo.toml` の `wasm-bindgen = "=0.2.130"` と devShell の `wasm-bindgen-cli` を一致させる。ずれると生成物が壊れる。
- `src/pkg/` と `public/` は生成物（gitignore 済み）。CI は build の成否と、生成された bundle が期待した wasm-bindgen で作られたことだけを見ればよい。

**deploy**

- `wrangler deploy`（または `wrangler versions upload` + `wrangler versions deploy`）を CI から実行する。必要なのは `CLOUDFLARE_API_TOKEN` と `CLOUDFLARE_ACCOUNT_ID`。
- token の scope は最小限: **Workers Scripts: Edit** + 対象 zone の **Workers Routes: Edit**（カスタムドメイン維持）。Access や DNS の変更はこの token に持たせない（別作業）。
- `wrangler.toml` の `CLUSTER` / `NODE_ID` / `EPOCH` / `ORIGIN` は公開情報としてレビュー対象にする。**deploy で identity を変えない**。CI は `NODE_ID` が既存と一致することを確認する。
- staging は初期は常設しない。DO migration を入れる回だけ、別 Worker 名・別 `NODE_ID` / `SEED` / domain・別 DO で一時構築する。`wrangler` の env（`[env.staging]`）か別 toml で分け、migration は env ごとに適用される点に注意。
- prod は GitHub Environment（required reviewers）経由。`workflow_dispatch` でも同じ Environment を通す。

**実行時 secret**

- `SEED` / `ADMIN_TOKEN` / `BOOTSTRAP_TOKEN` は **owner が 1Password から一度だけ `wrangler secret put`** する。CI は書き換えない。
- deploy 後の smoke test で `/state` と `/ready` を確認する。`/ready` が 503 なら設定不足。`NODE_ID` の照合が必要なら、公開してよい識別子だけを返す `/version` を追加する（実装は後で）。
- `ADMIN_TOKEN` を使う管理 API の確認は Access の内側なので、CI からやるなら Access service token が別途必要。最初は `/state` / `/ready` だけで十分。
- `BOOTSTRAP_TOKEN` は初回 provision 後すぐ削除する。CI の deploy 手順に含めない（誤って再 provision しないため）。

**rollback**

- `wrangler versions deploy <previous-version>@100%` か `wrangler rollback`。assets とコードは戻せるが、**DO の schema と保存データは戻らない**。migration を含む変更は一時 staging か workerd probe で先に踏む。
- 失敗時の自動 rollback はしない（データの前方互換性を壊す可能性）。smoke が落ちたら deploy を止め、owner が判断する。

## 7. シークレット

### 7.1 原則

1. **1Password を正**にする。GitHub には「1Password を読むための token」だけを置く。
2. **deploy 用の token は GitHub Environment secret** に置く（1Password 障害で deploy が止まらないように）。PR からは見えない。
3. **runtime secret は CI が触らない**。Worker の `SEED`、ノードの鍵、keystore は owner の操作でのみ更新する。
4. **Nix store に秘密を入れない**。Nix store は world-readable。sops-nix で復号した値を derivation に渡すのも避ける。
5. **ログに出さない**。`set -x` を署名・secret 取得の前後で使わない。`op` の取得値は `::add-mask::` する。

### 7.2 置き場所

| 名前 | 用途 | 置き場所 | 注入先 | 備考 |
|---|---|---|---|---|
| `OP_SERVICE_ACCOUNT_TOKEN` | 1Password の読み取り | GitHub Environment `release-android` / `release-worker` の secret | CI job の env | 専用 vault の read-only。90 日で rotate |
| `CLOUDFLARE_API_TOKEN` | `wrangler deploy` | GitHub Environment `cloudflare-prod`（staging を作る場合だけ `cloudflare-staging`） | CI job の env | Workers Scripts:Edit + Routes:Edit のみ。四半期で rotate |
| `CLOUDFLARE_ACCOUNT_ID` | account 指定 | GitHub の vars（secret でなくてよい） | CI job の env | 公開情報 |
| `CACHIX_AUTH_TOKEN` | `satler` cache への push | GitHub repo secret | main / tag の job のみ | PR では使わない |
| `SEED` | Worker の node identity | **1Password のみ** | owner が `wrangler secret put` | CI は読まない・書かない |
| `ADMIN_TOKEN` | Worker 管理 API | 1Password のみ | owner が `wrangler secret put` | smoke で使うなら CI が読む（mask） |
| `BOOTSTRAP_TOKEN` | 初回 provision | 1Password のみ | owner が `wrangler secret put` → 削除 | CI に渡さない |
| release keystore + パスワード | APK 署名 | 1Password Document（アプリごと） | CI が一時取得 | `cert_sha256` も同じ item |
| Play upload key（使う場合） | AAB 署名 | 1Password | CI が一時取得 | app signing cert は Play Console |
| Access service token（任意） | 管理 API の smoke | 1Password | CI が一時取得 | 最初は不要 |

**1Password の vault 設計**: 個人の `dev` vault 全体を CI に読ませない。`harmion` のような専用 vault を作り、必要な item だけ置く。service account はその vault の read-only にする。keystore の Document もこの vault に移す（人間の owner は両方見られる）。

### 7.3 CI での扱い

```bash
set -euo pipefail
umask 077
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# 1Password から取得（値は標準出力に出さない）
op document get "Harmion release keystore" --vault harmion \
  --out-file "$work/release.jks"
chmod 600 "$work/release.jks"

password="$(op item get "Harmion release keystore" --vault harmion \
  --fields label=pass --reveal)"
echo "::add-mask::$password"
```

- `op run --env-file` を使うなら、テンプレートに `op://harmion/...` を書き、値は env にだけ入れる。テンプレート自体はコミットしてよい（値は入らない）。
- keystore は job の最後に必ず削除する。GitHub の workspace は job 終了で破棄されるが、`$RUNNER_TEMP` と artifact upload の対象を確認する。
- secret を `echo` して確かめない。`set -x` を有効にしない。`op` のエラーに item 名が出ることはあるが、値は出さない。
- GitHub の secret masking は「GitHub に登録した secret」だけ。`op` で取ってきた値は自動では mask されないので、必ず `::add-mask::` する。

### 7.4 やってはいけないこと

- `pull_request_target` で secret を使う（fork のコードが走る）。
- PR の workflow に Environment secret を渡す。
- `wrangler secret put` を CI の通常 deploy に含める（`SEED` を誤って上書きすると node identity が変わる）。
- keystore を Nix derivation の `src` や `buildInputs` に入れる。
- `harmion-admin-root.pem` や `.deploy-secrets/worker.env` をリポジトリ直下に置き続ける。
- 署名済み APK を artifact として PR に添付する（誰でも取得できる。Release のみ）。

### 7.5 既存の後始末

- `harmion-admin-root.pem` と `.deploy-secrets/` の中身を 1Password の専用 vault に移し、作業ツリーから削除する（git 履歴には入っていない）。
- GitHub の secret scanning / push protection を有効にする（public repo なら無料）。
- `gitleaks` か `osv-scanner` を pre-commit と CI に入れる。
- 1Password の service account を作ったら、`op` の利用ログ（1Password のイベント）を確認できるようにする。

### 7.6 漏洩時の影響と rotate

| 秘密 | 漏洩したら | rotate |
|---|---|---|
| release keystore | 同一 applicationId の偽アプリを配布できる。**最重要** | 鍵を変えると更新できない。applicationId を変えるか、Play なら upload key を reset。オフラインバックアップを維持 |
| `SEED` | Worker の node を偽装できる | `NODE_ID` を変えて新しい DO を作り、native から再承認・再 provision。古い node は退役 |
| `ADMIN_TOKEN` | Worker の管理 API を叩ける（Access の内側なら影響は限定的） | 新しい値を `wrangler secret put`。1Password も更新 |
| `CLOUDFLARE_API_TOKEN` | Worker を改ざん・削除できる | Cloudflare で revoke → 新規発行 → GitHub Environment を更新 |
| `OP_SERVICE_ACCOUNT_TOKEN` | 対象 vault の secret を読める | service account を revoke → 再作成 |

## 8. Nix とインストールの境界

### 8.1 harmion が保証する Nix 出力

```nix
# harmion/flake.nix（将来）
packages.<system> = {
  default = ...;          # harmiond + harmion + harmion-admin の buildEnv
  harmion = ...;          # CLI
  harmiond = ...;         # daemon
  harmion-admin = ...;    # 管理 UI
  wiki-gui = ...;         # 任意（anti-webui の git 依存があるので別 package）
  wiki-lsp = ...;
  wiki-server = ...;
  harmion-backup = ...;   # script + unit
};

checks.<system> = { ... };
```

- `packages` は **純粋**に保つ（secret を読まない）。
- `default` は `harmiond` / `harmion` / `harmion-admin` と systemd unit をまとめた `buildEnv`。`harmion-update` はこれを 1 回の `nix build` で取る。
- `harmion-static` は今は作らない。非 Nix 環境向けの配布が必要になったら追加する。

### 8.2 インストールは dotfiles を介さない

`harmion-update` がタグを `nix build` し、profile を GC root にして `~/.local/bin` から symlink する。

```
~/.local/share/harmion/profiles/v0.1.0  -> /nix/store/...-harmion-0.1.0   (GC root)
~/.local/share/harmion/current          -> profiles/v0.1.0
~/.local/share/harmion/previous         -> profiles/v0.0.9
~/.local/bin/harmiond                   -> ../share/harmion/current/bin/harmiond
~/.local/bin/harmion                    -> ../share/harmion/current/bin/harmion
~/.local/bin/harmion-admin              -> ../share/harmion/current/bin/harmion-admin
~/.local/bin/harmion-update             -> ../share/harmion/current/bin/harmion-update
```

- `profiles/<tag>` は `nix build --out-link` の後、`nix-store --add-root` で GC root として登録する。out-link を `mv` すると auto root が切れるため。
- `current` を付け替えるだけで version が変わる。旧 profile を残せば `--rollback` できる。
- 初回は既存の `~/.local/bin/harmion*`（実ファイル）を `~/.local/share/harmion/pre-install/` に退避してから symlink に置き換える。
- systemd unit は `harmion-update --systemd` で `~/.config/systemd/user/` に置く。`--restart` のときだけサービスを再起動する。

### 8.3 なぜ musl を使わないか

今 `~/.local/bin/harmiond` に入っているバイナリも、interpreter は `/nix/store/...-glibc-2.44-57/...` を指している（`readelf -l` で確認済み）。つまり **すでに Nix store に依存した動的バイナリを直接置いている**。

musl static が必要になるのは、GitHub Release のバイナリを Nix のないマシンへ配る場合。今回は「このマシンで Nix ビルドしたものを `~/.local/bin` に置く」なので、静的化せず、**profile の out-link を GC root にして store path を保持する**だけでよい。

- store path が GC されなければ、動的バイナリのまま動く。
- マシン移動や Nix なし環境への配布が必要になったら、そのとき `harmion-static`（musl）を追加する。
- glibc + nix-ld に頼る案は、nix-ld の提供ライブラリに依存するので採らない。

### 8.4 移行の段階

1. **今**: dotfiles は触らない。harmion に CI・Nix packages・`harmion-update` を足す。
2. **最初のタグ**: `v0.1.0` を切る。`harmion-update --version v0.1.0` で `~/.local/bin` に入れ、`harmiond --version` を確認。
3. **常用**: daemon の unit は既存の `~/.local/bin/harmiond` を指したままでよい。更新は `harmion-update`。
4. **将来**: dotfiles に入れたくなったら、そのとき改めて flake input か binary derivation を検討する。今は決めない。


## 9. 失敗時・ロールバック

| レーン | 戻し方 | 注意 |
|---|---|---|
| native | `harmion-update --rollback` で `current` を前の profile に戻す | DB の schema migration は前方のみ。旧バイナリで新 schema を読めるか確認（[linux-service.md](linux-service.md)） |
| Android | 同じ applicationId・同じ鍵で `versionCode` を上げて再リリース | `versionCode` は下げられない。署名鍵を失うと更新不可。Release の削除は端末の更新を戻さない |
| Worker | 1 つ前の version を deploy（`wrangler versions deploy` / `wrangler rollback`） | DO の schema・データは戻らない。migration は一時 staging か workerd probe で検証 |

- Release の asset は上書きしない（immutability）。間違えたら新しい patch を切る。
- Worker の prod deploy が smoke で落ちたら、自動で戻さず owner に通知して停止する。データの前方互換性を壊す変更は自動 rollback と相性が悪い。

## 10. 実装順

実行順・成果物・完了条件は [ci-plan.md](ci-plan.md) にまとめた。要約すると次の順序。

1. **Phase 0**: 版の一元化、`version.properties`、`release-apps.toml`、JNI の Gradle 接続、admin UI の npm 分離、secret の移動
2. **Phase 1**: flake に `packages` を追加（`default` は 3 つの buildEnv）
3. **Phase 2**: PR CI（rust / nix / android / worker / secret scan）
4. **Phase 3**: `harmion-update` を追加。タグから `nix build` して `~/.local/bin` に symlink。Cachix に push
5. **Phase 4**: `node-v*` / `mote-v*` タグで Android の署名 APK を Release。Obtainium で更新確認
6. **Phase 5**: `vX.Y.Z` タグ / 手動で Worker を prod へ deploy。smoke と rollback を確認

## 11. 決めること

1. **native の入れ方**: タグから `nix build` して `profiles/<tag>` + `~/.local/bin` symlink（推奨）か、`~/.local/bin` に実ファイルを置くか。
2. **最初のタグ**: `v0.1.0` か、CI 整備後の最初を `v0.2.0` にするか。
3. **1Password の vault 名**: `harmion` でよいか。
4. **native の aarch64**: 当面 x86_64 のみか、必要になったら追加するか。
5. **SDK の外部配布**: リポジトリ内の `project(":sdk")` で足りる間は AAR を出さない、でよいか。
6. **Worker の staging**: 初期は prod のみ + migration 時だけ一時 staging、でよいか。

## 付録 A: workflow の骨子

以下は形を示すための最小例。実際には action を SHA で pin し、`timeout-minutes` と `concurrency` を付ける。

### A.1 PR CI

```yaml
name: ci
on:
  pull_request:
  push:
    branches: [main]
concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true
permissions:
  contents: read

jobs:
  rust:
    runs-on: ubuntu-24.04
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@<sha> # v7
      - uses: cachix/install-nix-action@<sha> # v31
        with:
          extra_nix_config: |
            experimental-features = nix-command flakes
            accept-flake-config = true
      - uses: cachix/cachix-action@<sha> # v17
        with:
          name: satler
          authToken: ${{ secrets.CACHIX_AUTH_TOKEN }}
          push: ${{ github.event_name == 'push' }}
      - run: |
          nix develop --command bash -euo pipefail -c '
            npm ci --prefix apps/admin/ui
            cargo fmt --all -- --check
            cargo clippy --workspace --all-targets -- -D warnings
            cargo test --workspace --locked
          '

  worker-build:
    runs-on: ubuntu-24.04
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@<sha>
      - uses: cachix/install-nix-action@<sha>
      - run: |
          nix develop --command bash -euo pipefail -c '
            cargo fetch --locked
            deploy/cloudflare/build.sh
          '
```

### A.2 Android release（1 アプリ分）

```yaml
name: release-android
on:
  push:
    tags: ["node-v*", "mote-v*"]
permissions:
  contents: write

jobs:
  release:
    runs-on: ubuntu-24.04
    environment: release-android
    timeout-minutes: 60
    env:
      TAG: ${{ github.ref_name }}
    steps:
      - uses: actions/checkout@<sha>
        with:
          fetch-depth: 0
      - uses: cachix/install-nix-action@<sha>
      - uses: cachix/cachix-action@<sha>
        with:
          name: satler
          authToken: ${{ secrets.CACHIX_AUTH_TOKEN }}
          push: false
      - name: build and sign
        env:
          OP_SERVICE_ACCOUNT_TOKEN: ${{ secrets.OP_SERVICE_ACCOUNT_TOKEN }}
        run: |
          nix develop .#android --command bash -euo pipefail -c '
            ./android/scripts/release-app.sh "$TAG"
          '
      - name: publish
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          set -euo pipefail
          shopt -s nullglob
          assets=(target/release/*"$TAG"*.apk target/release/*"$TAG"*-mapping.txt target/release/*"$TAG"*-cert-sha256.txt target/release/*"$TAG"*-SHA256SUMS)
          prerelease=()
          if [[ "$TAG" == *-rc.* ]]; then prerelease=(--prerelease); fi
          gh release create "$TAG" "${assets[@]}" --title "$TAG" --generate-notes "${prerelease[@]}"
```

`release-app.sh` は manifest を読み、version を検査し、1Password から keystore を取得し、JNI → Gradle → `apksigner verify` までを行う。workflow 自体はアプリを知らない。

### A.3 native cache / Worker deploy

native は GitHub Release を作らない。タグ時に `.#default` を Cachix に載せ、`harmion-update` のビルドを速くする。

```yaml
name: cache-native
on:
  push:
    tags: ["v*"]
permissions:
  contents: read

jobs:
  cache:
    runs-on: ubuntu-24.04
    timeout-minutes: 60
    steps:
      - uses: actions/checkout@<sha>
      - uses: cachix/install-nix-action@<sha>
      - uses: cachix/cachix-action@<sha>
        with:
          name: satler
          authToken: ${{ secrets.CACHIX_AUTH_TOKEN }}
          push: true
      - run: |
          set -euo pipefail
          nix build .#default --no-link --print-build-logs
          nix build .#default --no-link --print-out-paths | xargs cachix push satler
```

Worker は別 workflow（`.github/workflows/deploy-worker.yml`）で、`cloudflare-prod` Environment の承認を経て deploy する。

```yaml
name: deploy-worker
on:
  workflow_dispatch:
  push:
    tags: ["v*"]
permissions:
  contents: read

jobs:
  deploy:
    runs-on: ubuntu-24.04
    environment: cloudflare-prod
    timeout-minutes: 45
    steps:
      - uses: actions/checkout@<sha>
      - uses: cachix/install-nix-action@<sha>
      - run: |
          nix develop --command bash -euo pipefail -c '
            cargo fetch --locked
            deploy/cloudflare/build.sh
          '
      - run: nix develop --command wrangler deploy --cwd deploy/cloudflare
        env:
          CLOUDFLARE_API_TOKEN: ${{ secrets.CLOUDFLARE_API_TOKEN }}
          CLOUDFLARE_ACCOUNT_ID: ${{ vars.CLOUDFLARE_ACCOUNT_ID }}
      - run: |
          curl -fsS https://hub.satler.dev/state
          curl -fsS https://hub.satler.dev/ready
```

## 付録 B: 参考コマンド

```sh
# 署名の確認
apksigner verify --print-certs target/release/harmion-release.apk

# 証明書の SHA-256（keystore から）
keytool -exportcert -keystore release.jks -alias harmion -storepass "$PASS" -file cert.der
sha256sum cert.der

# Worker の deploy 履歴と rollback
nix develop --command wrangler deployments list --cwd deploy/cloudflare
nix develop --command wrangler rollback --cwd deploy/cloudflare

# Cachix へ push（例）
nix build .#harmiond .#harmion .#harmion-admin --no-link --print-out-paths \
  | xargs cachix push satler
```
