# Android エミュレータでの検証手順

Harmion AndroidのUI・承認・参加承認・release APKを、API 36のエミュレータで確認する
手順をまとめる。実機の代わりに使うが、生体認証・Doze・着信は実機確認が必要。

## 1. 前提

```bash
cd /path/to/harmion
nix develop .#android
echo "$ANDROID_HOME"   # nixが供給するSDK
```

- システムイメージ: `system-images;android-36;google_apis;x86_64`
- AVD名: `harmion`（Pixel 6相当 / 1080x2400 / x86_64）
- このリポジトリではAVDを `target/android-avd/` に置いている（git管理外）。

## 2. AVDの作成と起動

既存のAVDを使う場合:

```bash
export ANDROID_AVD_HOME="$PWD/target/android-avd"
emulator -avd harmion -no-snapshot -no-boot-anim -gpu swiftshader_indirect
```

画面なしで操作する場合:

```bash
emulator -avd harmion -no-window -no-snapshot -no-boot-anim -gpu swiftshader_indirect &
adb wait-for-device
```

新規作成する場合（システムイメージは`sdkmanager`で導入済みにする）:

```bash
export ANDROID_AVD_HOME="$PWD/target/android-avd"
mkdir -p "$ANDROID_AVD_HOME"
avdmanager create avd \
  -n harmion \
  -k "system-images;android-36;google_apis;x86_64" \
  -d pixel_6 \
  --path "$ANDROID_AVD_HOME/harmion.avd"
```

起動完了を待つ:

```bash
adb wait-for-device
until [ "$(adb shell getprop sys.boot_completed | tr -d '\r')" = "1" ]; do sleep 2; done
```

複数デバイスがある場合は `adb -s emulator-5554 ...` のように指定する。

## 3. ロック解除と共通操作

検証用PINを設定する（本番端末のPINではない）:

```bash
adb shell locksettings set-pin 1234
adb shell input keyevent 82
adb shell input swipe 540 2000 540 500 200
adb shell input text 1234
adb shell input keyevent 66
```

画面ダンプとタップ:

```bash
mkdir -p target/emulator
ui() {
  adb shell uiautomator dump /sdcard/ui.xml >/dev/null 2>&1
  adb pull /sdcard/ui.xml target/emulator/ui.xml >/dev/null 2>&1
}
ui
grep -o 'text="[^"]*"[^>]*bounds="[^"]*"' target/emulator/ui.xml | head
adb shell input tap 540 1200
```

座標は画面やダイアログで変わる。**必ずダンプしてboundsを確認してからタップする**。

生体認証プロンプトにはエミュレータの指紋を使う:

```bash
adb -e emu finger touch 1
```

## 4. アプリの導入と初期化

```bash
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
adb install -r android/testclient/build/outputs/apk/debug/testclient-debug.apk
adb shell am start -n dev.satler.harmion/org.harmion.android.ui.MainActivity
```

初回は通知許可ダイアログが出る。`Allow` を押してからオンボーディングへ進む。

- 「この端末だけで始める」→ ノード作成
- 「既存のクラスタに参加」→ 招待QR読み取り、または「招待コードを貼り付け」

アプリのデータを消してやり直す:

```bash
adb shell pm clear dev.satler.harmion
```

## 5. 所有者登録と承認E2E

### 所有者登録

1. 設定タブ → 「所有者キーを登録」
2. 生体認証プロンプトで `adb -e emu finger touch 1`
3. リカバリコード画面を確認して閉じる

### testclientからの承認

`testclient` はdebug専用。配布しない。

```bash
adb shell am force-stop org.harmion.testclient
adb shell am start -n org.harmion.testclient/.MainActivity
```

1. 「接続を要求（READ/WRITE, 90日）」
2. ノード側の承認画面で差分・期限を確認して承認
3. 指紋認証
4. testclientへ戻り、`status OK: running=... membership=...` を確認

承認画面がノードのタスクに残らないこと、戻り先が要求元アプリであることを
`dumpsys`で確認する:

```bash
adb shell dumpsys activity activities | grep -E "ResumedActivity|topResumed"
```

## 6. 2ユーザーでの参加承認E2E

1台のエミュレータで Android ユーザーを分け、既存ノードAと参加ノードBを動かす。
アプリのデータはユーザーごとに独立している。

### 6.1 ユーザーBの作成

```bash
adb shell pm create-user HarmionB        # 例: id=10
adb shell am switch-user 10
adb shell settings --user 10 put global device_provisioned 1
adb shell settings --user 10 put secure user_setup_complete 1
adb install --user 10 -r android/app/build/outputs/apk/debug/app-debug.apk
adb shell am start --user 10 -n dev.satler.harmion/org.harmion.android.ui.MainActivity
```

注意:

- `adb install` は `--user` を `-r` より前に置く（`-r --user` は失敗する）。
- ユーザー切替後、ユーザー0はロックされる。戻ったらPINで解除する。
- `run-as` は切替中でもユーザー0のデータを指す。ユーザー10のprefsはrootで読む（後述）。

### 6.2 招待コードをBへ渡す

既存ノードAの `peer.json` を取得し、`PeerCode` 形式（gzip + base64url）に変換する:

```bash
adb exec-out run-as dev.satler.harmion \
  cat files/harmion/node/peer.json > target/emulator/peerA.json
```

```bash
python3 - <<'PY'
import base64, gzip, pathlib
data = pathlib.Path('target/emulator/peerA.json').read_bytes()
code = 'harmion-peer:v1:z:' + base64.urlsafe_b64encode(
    gzip.compress(data, mtime=0)).decode().rstrip('=')
pathlib.Path('target/emulator/inviteA.txt').write_text(code)
print(len(code))
PY
```

Bのオンボーディングで「招待コードを貼り付け」を開き、コードを入力する。

**長いコードは `adb shell input text` が途中で切れる**（実測: 約86文字）。
80文字ずつに分けて入力する:

```bash
paste_code() {
  local rest="$1"
  while [ -n "$rest" ]; do
    adb shell input text "${rest:0:80}"
    rest="${rest:80}"
    sleep 0.5
  done
}
paste_code "$(cat target/emulator/inviteA.txt)"
```

キーボードを閉じてから「参加」を押す。参加後は「参加承認待ち」QRが表示される。

### 6.3 参加要求コードをAへ渡す

通常はBのQRをAで読み取るか、共有コードを貼り付ける。自動化ではBの保存済み
参加要求コードをrootで取得できる:

`google_apis`（userdebug）イメージでのみ `adb root` が使える。

```bash
adb root
adb exec-out cat /data/user/10/dev.satler.harmion/shared_prefs/org.harmion.pending_join.xml
```

`<string name="code">…</string>` の中身を取り出し、Aの
「参加要求を承認」→「参加コード」に貼り付ける。

### 6.4 承認

1. Aのピアタブ → 「参加要求を承認」
2. 参加コードを貼り付け（またはQR読み取り）
3. クラスタID・ノードIDを確認して「承認」
4. `adb -e emu finger touch 1`
5. メンバー数が増えることを確認

承認後、B側で同期すると `membership=active` になり、「参加承認待ち」カードが
自動で消える。

## 7. 参加要求カードと再生成の確認

- Bで参加後、ホーム／ピアに「参加承認待ち」カードが出る
- アプリを強制終了→再起動してもQRが残る
- 表示時に `identity()` から現在のbinding/endpointでコードを再生成する

endpoint変更の追随を確認する場合は、保存コードの `relay_url` などを古い値に
書き換えてから起動する（rootでprefsを上書きする場合）:

```bash
adb push stale_pending.xml /data/local/tmp/stale_pending.xml
adb shell "cp /data/local/tmp/stale_pending.xml \
  /data/user/10/dev.satler.harmion/shared_prefs/org.harmion.pending_join.xml"
```

上書き後は所有者・SELinuxコンテキストを戻す:

```bash
APPUID=$(adb shell stat -c %u /data/user/10/dev.satler.harmion)
adb shell "chown $APPUID:$APPUID \
  /data/user/10/dev.satler.harmion/shared_prefs/org.harmion.pending_join.xml"
adb shell "chmod 660 \
  /data/user/10/dev.satler.harmion/shared_prefs/org.harmion.pending_join.xml"
adb shell "restorecon \
  /data/user/10/dev.satler.harmion/shared_prefs/org.harmion.pending_join.xml"
```

起動後、カードの「更新」または自動再生成で現在の `identity()` の値に戻る。
root化した場合は最後に `adb unroot` で戻す。

## 8. release APKの確認

署名鍵の取得は [android-release.md](android-release.md) を参照。エミュレータでは
一時鍵で署名・インストールだけ確認できる（実鍵をコミットしない）:

```bash
HARMION_KEYSTORE=/path/to/test.jks \
HARMION_KEYSTORE_PASSWORD=... \
HARMION_KEY_ALIAS=harmion \
HARMION_KEY_PASSWORD=... \
./android/scripts/release-1password.sh
```

- debugとreleaseは同じ `dev.satler.harmion`。署名が違うため共存できない。
  先に `adb uninstall dev.satler.harmion` する。
- releaseのSDK証明書ピンは署名証明書のSHA-256に合わせる。
- 一時鍵・テストAPKは確認後に削除する。

## 9. トラブルシュート

| 症状 | 対処 |
|---|---|
| `Activity class does not exist` / `Error type 3` | 画面ロックを解除してから起動 |
| 入力したコードが途中で切れる | 80文字ずつ `paste_code` で分割入力 |
| `install: Unknown option 'r'` | `adb install --user 10 -r ...` の順序にする |
| 操作しても反応しない | `uiautomator dump` で現在の画面とboundsを確認 |
| テストクライアントが前面に残る | `adb shell am force-stop org.harmion.testclient` |
| ユーザー10のデータが見えない | `run-as`はユーザー0固定。rootで `/data/user/10/...` を読む |
| 承認後に戻り先が違う | `dumpsys activity activities` で `ResumedActivity` を確認 |
| releaseが起動直後に落ちる | R8のkeep rule（WorkManager/Room）と `assembleRelease` を確認 |
| `adb root` 後に接続が切れる | `adb wait-for-device`、最後は `adb unroot` |

## 10. 証跡と後片付け

```bash
mkdir -p target/emulator
adb exec-out screencap -p > target/emulator/screen.png
adb shell uiautomator dump /sdcard/ui.xml
adb pull /sdcard/ui.xml target/emulator/ui.xml
```

`target/` はgit管理外。画面証跡にリカバリコード・招待コード・個人情報が
映らないよう確認する。検証後は一時鍵・APK・UIダンプを削除する。
