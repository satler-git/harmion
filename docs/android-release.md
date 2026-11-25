# Harmion Android — リリースと署名

## 1. 識別子
- applicationId: `dev.satler.harmion`
- code namespace: `org.harmion.android`（サービス/AIDL/Activityのクラス名はこのまま。
  SDKは `dev.satler.harmion/org.harmion.android.service.HarmionNodeService` を bind する）
- 署名を変えると同一applicationIdの更新としてインストールできない。release鍵は
  **アプリの寿命と同じ**期間保持する。

## 2. 鍵の保管（1Password）
1Passwordの **1つのDocumentアイテム** にkeystoreとメタデータをまとめる。

| 項目 | 値 |
|---|---|
| Vault | `dev`（`OP_VAULT`で変更可） |
| アイテム | `Harmion release keystore`（Document） |
| ファイル | `harmion-release.jks` |
| フィールド `pass` | keystoreパスワード（concealed） |
| フィールド `key psas` | 鍵パスワード（concealed、通常は`pass`と同じ） |
| フィールド `alias` | 鍵エイリアス（`harmion`） |
| フィールド `cert_sha256` | 署名証明書のSHA-256 |

`release-1password.sh` は次の順で読み出す:
1. `op document get "Harmion release keystore" --vault dev --out-file ...`
2. `op item get ... --format=json` からラベルで `pass` / `key psas` / `alias` / `cert_sha256` を取得
3. keystoreから証明書SHA-256を再計算し、`cert_sha256` と不一致ならビルドを中止

フィールド名を変えた場合は `OP_PASSWORD_LABEL` / `OP_KEY_PASSWORD_LABEL` / `OP_ALIAS_LABEL` /
`OP_CERT_LABEL` で上書きできる。`key psas` が空の場合は `key_password` なども自動で試す。

## 3. 初回の鍵作成
```bash
nix develop .#android
./android/scripts/new-release-keystore.sh
```
- RSA 4096 / PKCS12 / 有効期限10000日 / alias `harmion`
- 生成後にパスワードと `cert_sha256` を表示する。**その場で1Passwordへ保存**し、
  オフラインバックアップも取る（表示は一度きり）。
- 既存ファイルは上書きしない。

1Passwordへ保存（スクリプトが表示するコマンド）:
```bash
op document create target/release-keystore/harmion-release.jks \
  --title "Harmion release keystore" --vault "dev"
op item edit "Harmion release keystore" --vault "dev" \
  "pass=..." "key psas=..." "alias=harmion" "cert_sha256=..."
```
保存後、作業ディレクトリのkeystoreは削除してよい（1Passwordが正）。

## 4. リリースビルド
1Password CLIのデスクトップアプリ連携を有効にした状態で:
```bash
nix develop .#android
./android/scripts/release-1password.sh
```
- 1Passwordからkeystoreを一時ディレクトリへ取得（0600、終了時に削除）
- `cert_sha256` とkeystoreの証明書を照合
- 証明書SHA-256を `-PharmionNodeCertSha256=<sha>` としてSDKのピンに注入
- `:app:assembleRelease` を実行し、`apksigner verify --print-certs` で照合
- 成果物: `target/release/harmion-release.apk`

バージョン:
```bash
HARMION_KEYSTORE=... HARMION_KEYSTORE_PASSWORD=... HARMION_KEY_ALIAS=harmion \
HARMION_KEY_PASSWORD=... gradle -p android :app:assembleRelease \
  -PharmionNodeCertSha256=<sha> -PharmionVersionCode=2 -PharmionVersionName=0.2.0
```

ローカル鍵でのテスト（実鍵をコミットしない）:
```bash
HARMION_KEYSTORE=/path/to/test.jks \
HARMION_KEYSTORE_PASSWORD=... HARMION_KEY_ALIAS=harmion HARMION_KEY_PASSWORD=... \
./android/scripts/release-1password.sh
```
`HARMION_KEYSTORE` が設定されていれば1Passwordは参照しない。

## 5. 証明書ピン（重要）
- SDKはノードアプリの署名証明書SHA-256を `BuildConfig.NODE_CERT_SHA256` で照合する。
- debugの既定値は開発用debug証明書。**release APKを配布する場合、クライアントアプリが使う
  SDKも同じrelease証明書のSHA-256でビルドする必要がある**:
  ```bash
  gradle :sdk:... -PharmionNodeCertSha256=<release sha>
  ```
- Play App Signingを使う場合、端末にインストールされる証明書はGoogleの**app signing
  certificate**であり、upload keyではない。ピンにはPlay Consoleの
  「アプリ署名証明書」のSHA-256を使う。upload keyは1Passwordで保管する。

## 6. インストールと更新
```bash
adb install -r target/release/harmion-release.apk
```
- debug APKとrelease APKは同じ `dev.satler.harmion`。署名が異なるため共存できず、
  `INSTALL_FAILED_UPDATE_INCOMPATIBLE` になる。releaseを試す前にdebugをアンインストールする
  （アプリデータも消える。ノードを保持したい場合は別端末/ユーザーで試す）。
- 更新は同じapplicationId・同じ署名鍵・より大きい `versionCode` が必須。
- 署名鍵を失うと更新不可。1PasswordのDocumentとオフラインバックアップを維持する。

## 7. リリース前チェックリスト
- [ ] `cargo test`（node / android / sdk-native / net-native / store-sqlite / runtime）
- [ ] `gradle :app:assembleDebug :testclient:assembleDebug`
- [ ] `./android/scripts/owner-smoke.sh`
- [ ] エミュレータで承認E2E（要求→差分/期限→指紋→接続完了→要求元アプリへ戻る）
- [ ] F-04: 未承認status拒否／HOME後のidle stop／次操作でのオンデマンド起動
- [ ] `versionCode` を更新
- [ ] `release-1password.sh` で署名・証明書照合
- [ ] APKを実機へインストールし、初期化・所有者登録・更新を確認
- [ ] タグ/CHANGELOG（任意）

## 8. CIでの利用
- `OP_SERVICE_ACCOUNT_TOKEN` を使う場合は、対象Vaultへのread-only権限を付与する。
- keystore Documentと署名アイテムを読めるサービスアカウントを作り、CIのsecretに保存する。
- 署名済みAPKと証明書SHA-256をアーティファクトに残す。パスワードやkeystoreはログに出さない。
