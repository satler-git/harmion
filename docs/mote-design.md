# Mote — テキスト Capture の実装と確認

[要件 v0.2](mote-requirements.md) の短文・長文テキスト Capture を実装する。Mote はノードの `:app` とは別 APK（`android/mote`、Gradle パス `:mote`、applicationId `dev.satler.mote`、Kotlin namespace `org.harmion.mote`）。公開ホスト・DNS・外部 API は不要で、Harmion の宛先をこのドメインに固定しない。

## 動作と責務

- **UI:** [Drift のダーク Material 3 Expressive](../android/mote/src/main/java/org/harmion/mote/MoteTheme.kt) を標準にする。ホームは日付で区切った時系列で、短文は右寄せの吹き出し、完了した長文は横幅いっぱいのプレビューに本文を最大 3 行表示する。下部に長文の再開、Inbox / All、ピン留めフィルター、入力をまとめる。長文画面は本文を全幅で表示し、共有リンクには同じテキストの下書きとして補足を加えられる。右上の設定には Harmion の接続操作、完了演出、アーカイブ時の触覚を置く。
- **操作:** タップで本文の選択ができる詳細を開き、長押しでピン留め・コピー・OS 共有・アーカイブ・削除にアクセスする。左スワイプは約 35% の距離を目安に、色の変化でアーカイブ／解除の確定を示す。確定後は一覧へ先に反映し、ノードが拒否したら戻す。Snackbar で取り消せる。削除は別の確認画面からだけ実行する。
- **ViewModel:** 下書き・保留・承認・購読・操作結果を UI に渡す。接続できない状態や承認待ち、読み込み、失敗を、スナップショット取得済みの空 Inbox と区別する。書き込みは一度に一操作。
- **CaptureRepository:** ノードを正本とする。`capture` Kind の read / write だけを要求し、Android SDK の署名検証済み Binder と証明書ピン付き HTTPS で接続する。独自同期や Wiki 操作は持たない。

## Harmion に保存する形

短文送信 1 回 = `capture` Kind の Content 1 件。Body は普通のテキスト。属性は `capture` 名前空間に保存する。内容に別の Kind が共存できるというノードの設計は変更しない。

| key | 値 | 規約 |
|---|---|---|
| `created_at_micros` | UTC epoch µs の整数 | 作成時に一度設定。日時なしの Content は先頭に置き、同順位は Content ID 順。 |
| `archived` | bool | 不在は false。Inbox は false のみ、All は両方。 |
| `pinned` | bool | 不在は false。Inbox / All と独立した絞り込み。 |
| `session_open` | bool | 長文の初回追記で true、明示完了で false。短文では不在。 |

長文セッションは最初の追記で Content 1 件を作成し、その後は同じ ID の本文に空行を挟んで追記する。作成端末の Mote がローカルに長文セッション 1 件を保持し、ホームへ戻る操作やアプリ終了では完了しない。他の端末では本文を閲覧できるが、そのセッションへ追記はできない。Mote から進行中の Content を削除することはできない。

`derived_from` は LLM 整形の導入時に定義する。音声認識中・LLM 実行中などの一時状態は Content に保存しない。

## 接続・書き込みの境界

1. `observe` の `SnapshotEnd` まで一覧を表示せず、その後は `Changes` を ID で反映する。`ResetRequired` で一覧・詳細・操作確認を閉じ、次の完全なスナップショットから再開する。1 回の購読で 3 回までリセットを許容する。一時的な通信断では一覧購読だけを最大 2 回再試行し、書き込みは再送しない。復帰できない場合は手動再接続にする。現行 SDK は全本文を受け取るため、大規模 Inbox のメモリ負荷は未評価。
2. 下書きはバックアップ対象外のアプリ専用領域へ保存する。短文送信時は固定 ID と認可主体付きの create 要求を送信前に永続化し、保存の応答を受けるまで下書きを消さない。再送は明示操作だけ。`view: null` でもローカル書き込みに成功し得る。端末間同期は待たない。[失敗時の詳細](mote-recovery.md)と[保存契約](../contracts/app-save-v1.md)を参照。
3. ピン留め／アーカイブは現在の可視性と属性値を `get` で再確認し、該当属性だけ変更する。削除は再確認後に、確認画面の編集トークンを使用する。別 Kind が見える Capture や可視競合のある Capture は削除しない。`get` と書き込みの間に競合し得るため、条件付き delete と同等ではない。Phase 1 では派生を生成せず、連鎖削除もしない。

## 起動・確認

リポジトリのルートから（他の Android モジュールと同じツールチェーン）:

```sh
nix develop .#android
gradle -p android :mote:assembleDebug :mote:testDebugUnitTest
# Android 10 以降の検証端末またはエミュレータ
adb install -r android/mote/build/outputs/apk/debug/mote-debug.apk
adb shell am start -n dev.satler.mote/org.harmion.mote.MainActivity
```

対応する署名のノードを導入する。JNI の生成は [build-rust.sh](../android/scripts/build-rust.sh)、ノードの証明書ピンは [署名手順](android-release.md) を参照。動作確認は、`capture` 限定の承認 → 短文を保存 → 再起動して表示 → アーカイブ／取り消し → 確認付き削除の順が分かりやすい。

長文の確認は、追記 → 次の下書きを入力 → アプリを終了・再起動 → 「長文を再開」 → 明示完了、の順で行う。再起動後は同じ本文と下書きが復元される。「完了」は残りの下書きも追記して閉じ、保存の照合が終わったら Inbox に戻って短い粒子のフィードバックを表示する。Harmion への保存結果が不明なら、本文／状態が一致するときだけ利用者が採用できる。照合は追記や完了を再送しない。詳しくは [復旧設計](mote-recovery.md#2-長文追記の直列化と終了境界) を参照。

ほかのアプリからテキスト／URL を OS 共有すると、Mote は専用の短文入力画面だけを開く。共有内容に補足を足して保存するか、キャンセルして呼び出し元に戻れる。共有の下書きと保留はホームの下書きとは別に保存する。

音声入力は [Phase 3 の候補比較とデバッグ版の録音検証](mote-phase3.md) を参照。

**未実装:** 音声認識、右スワイプを含む LLM 整形、CLI。未実装の操作ボタンは表示しない。
