# サードパーティライセンス表記

依存 OSS のライセンス要件を満たすため、配布物ごとにサードパーティ通知を生成して同梱する。MIT / BSD / Apache-2.0 などの表示義務は、著作権表示とライセンス全文を配布物に含めることで満たす。画面がない配布物はテキストを同梱し、Android は同じデータをライセンス画面に表示する。

## 対象と同梱方法

| 配布物 | 生成元 | 同梱 |
|---|---|---|
| `harmion` CLI | `apps/cli/Cargo.toml` | バイナリに埋め込み、`harmion licenses` で表示。Nix では `share/licenses/harmion/` にも配置 |
| `harmiond` | `apps/daemon/Cargo.toml` | Nix の `share/licenses/harmiond/` |
| `harmion-admin` | `apps/admin/Cargo.toml` | Nix の `share/licenses/harmion-admin/` |
| `wiki-gui`（デスクトップ） | `apps/wiki-gui/Cargo.toml` | バイナリに埋め込み、`wiki-gui --licenses` で表示 |
| Harmion node APK | Gradle 依存 + Android ネイティブ Rust | このアプリについて → ライセンス |
| Mote APK | Gradle 依存 + Android ネイティブ Rust | 設定 → Mote について → ライセンス |

将来の `mote-desktop` も `wiki-gui` と同じ方法（cargo-about の出力を埋め込み、`--licenses` または About 画面から表示）を使う。CLI / デスクトップでライセンス画面を作る必要はない。標準出力に全文を出せれば、テキストを同梱する要件を満たせる。

## 使う道具

自作の収集器は作らない。

- Rust: [cargo-about](https://github.com/EmbarkStudios/cargo-about)。`Cargo.lock` と各クレートのライセンスファイルから、クレート名・バージョン・ライセンス全文を生成する。`about.toml` で許可ライセンスと対象ターゲットを固定する。
- Android: [AboutLibraries](https://github.com/mikepenz/AboutLibraries) v15。ビルド時に Gradle の依存を収集し、POM と同梱ライセンスからメタデータを作る。Compose の `LibrariesContainer` が一覧と全文表示を提供する。
- Android のネイティブ部分: AboutLibraries は Gradle 外の Rust 依存を収集しない。cargo-about の結果を AboutLibraries の設定 JSON に 1 エントリとして渡す。

`cargo-about` はワークスペース全体ではなく `--manifest-path` ごとに実行する。これで CLI に GUI の `wgpu` 依存が混ざるのを避けられる。ターゲットは Linux（x86_64 / aarch64）と Android（arm64-v8a / x86_64）の和集合で、ある配布物には入らないターゲット固有クレートも含み得る。過剰掲載は安全側なので許容する。

## 生成

```sh
nix develop --command scripts/generate-third-party-notices.sh
```

生成物はコミットする。

```text
about.toml
about.hbs                         # 単体配布物用テンプレート
about-component.hbs               # Android ネイティブの連結用
licenses/harmion/THIRD-PARTY-NOTICES.txt
licenses/harmiond/THIRD-PARTY-NOTICES.txt
licenses/harmion-admin/THIRD-PARTY-NOTICES.txt
licenses/wiki-gui/THIRD-PARTY-NOTICES.txt
licenses/android-native/THIRD-PARTY-NOTICES.txt
android/licenses/libraries/harmion-native.json
android/licenses/licenses/harmion-native.json
```

`cargo fetch --locked` のあと `--offline` で生成するため、ライセンス文の取得に外部サービスを使わない。`Cargo.lock` が同じなら出力は再現する。

確認:

```sh
nix develop --command scripts/generate-third-party-notices.sh --check
```

CI ではこのコマンドだけを実行し、差分があれば失敗させる。`Cargo.lock` や Gradle 依存を変えたら生成物も更新する。

## Android

`android/app` と `android/mote` は `com.mikepenz.aboutlibraries.plugin.android` を適用する。ビルド時に `res/raw/aboutlibraries.json` が生成され、APK に含まれる。AGP 9 のリソース縮小で消えないよう `res/raw/keep.xml` で保持する。

画面は次の 2 つ。

- Harmion node: `AboutActivity` の「ライセンス」→ `LicensesActivity`
- Mote: `MoteAboutScreen` の「ライセンス」→ `MoteLicensesScreen`

どちらも `produceLibraries(R.raw.aboutlibraries)` と `LibrariesContainer` を呼ぶだけで、一覧・検索・ライセンス全文表示は AboutLibraries が持つ。独自のライセンス一覧や JSON パーサは持たない。行をタップすると `LibraryDetailMode.Dialog` でローカルの条文を開く。

`License.url` は `aboutLibraries.export.excludeFields` で出力から除外する。AboutLibraries はライセンス URL があると「ライセンスを見る」でブラウザを開くため、URL を残すとオフラインで条文を読めない。条文は `aboutlibraries.json` に `content` として入っており、URL を消すことで同じ画面内のダイアログに全文を出す。

Gradle 依存は AboutLibraries が自動収集する。Rust のネイティブ依存は `scripts/generate-third-party-notices.sh` が `android/licenses/` に設定 JSON として書き、各アプリの `aboutLibraries.collect.configPath` が読む。ネイティブ分は「Harmion native」1 エントリにまとめ、全文をそのライセンスとして表示する。

`LibrariesContainer` の UI 文言は英語の既定値（ライブラリ名・ライセンス名）で、画面名と確認ボタンだけ日本語にする。利用者にとって必要な情報は依存名・バージョン・ライセンス・全文であり、説明文は足さない。

## ライセンス方針

許可する SPDX は `about.toml` の `accepted` に列挙する。現在は MIT、Apache-2.0（LLVM exception 含む）、BSD-1/2/3-Clause、0BSD、ISC、Zlib、Unicode-3.0、Unlicense、CC0-1.0、CDLA-Permissive-2.0。コピーレフト（GPL / LGPL / MPL / EPL）は許可しない。

新しい依存が未許可のライセンスを使う場合、生成は失敗する。ライセンスを確認してから `accepted` に追加するか、依存を差し替える。`OR` 表記の依存は許可済みの選択肢で足りるため、使わないライセンスを追加する必要はない。

`ring` と `rustls` は cargo-about の既知の workaround を使う。それ以外のクレートでライセンスファイルの検出に失敗した場合だけ、`about.toml` のクレート別 `clarify` を追加する。手作業のライセンス表を増やさない。

## 未対応

- `harmion-admin` の Svelte UI（npm 依存）は Rust の通知に含まれない。`apps/admin/ui/package-lock.json` から標準ツール（`generate-license-file` など）で別途生成し、`harmion-admin` の同梱物に追加する必要がある。
- Cloudflare Worker（`crates/worker`）は利用者に配布するバイナリではないため対象外。必要になった時点で `--manifest-path crates/worker/Cargo.toml` と wasm ターゲットを追加する。
- Gradle 依存のライセンスは POM と JAR/AAR 内のファイルに依存する。POM が `NOASSERTION` の依存が増えた場合は、AboutLibraries の `config/licenses` で補う。
- `glance-appwidget-external-protobuf`（BSD-3-Clause）は POM が著作権者行のない SPDX テンプレートしか提供しない。条文はローカルで読めるが、著作権者表示を厳密に求める場合は `android/licenses` で個別に上書きする。
- APK 内のネイティブ分は 1 エントリにまとめている。クレート単位で検索したい場合だけ、cargo-about の JSON からクレート別エントリを生成する。

## 受け入れ確認

- `harmion licenses` と `wiki-gui --licenses` がライセンス全文を出力する。
- 両 APK の設定からライセンス画面が開き、AndroidX と Harmion native の両方が見える。
- 機内モードでも、行をタップしてライセンス全文を読める。
- `scripts/generate-third-party-notices.sh --check` が成功する。
- `Cargo.lock` を変えた状態で `--check` が失敗する。
- リリース APK で `aboutlibraries` リソースが縮小されずに残る。
