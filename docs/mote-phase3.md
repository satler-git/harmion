# Mote Phase 3 — 音声入力の検証

[要件 v0.2 §§2.3–2.5、10.3、12.2](mote-requirements.md) と [復旧設計 §3](mote-recovery.md) を前提とする。最初の評価言語は日本語と仮定する。実機で録音・認識を確認し、短文、長文の順に Capture へ統合する。

## 認識エンジン

| 候補 | 検証での位置づけ |
| --- | --- |
| Moonshine Japanese Tiny / Small Streaming | Android で比較するストリーミング候補。MIT のモデルがあり、Android のライブラリにアプリが取得した PCM を渡せる。途中結果と確定結果の更新、発話の開始時刻・長さを Mote の録音サンプル位置へ対応付けられるかを確認する。[モデル一覧](https://github.com/moonshine-ai/moonshine/blob/main/docs/models/available-models.md) · [PCM 入力と結果イベント](https://github.com/moonshine-ai/moonshine/blob/main/docs/using/transcription.md) |
| Vosk small-ja | 同じ PCM を渡して比較する軽量候補。公開モデルは 48 MB、Apache-2.0。製品名・固有名詞・英数字交じりのメモでの精度と、区間境界の取得を確認する。[モデル一覧](https://alphacephei.com/vosk/models#japanese) · [Recognizer API](https://github.com/alphacep/vosk-api/blob/master/java/lib/src/main/java/org/vosk/Recognizer.java) |
| sherpa-onnx / ReazonSpeech k2 | 日本語の精度比較と、録音済み区間の再認識候補。sherpa-onnx はこのモデルをオフライン認識モデルとして提供している。音声区間検出による疑似ストリーミングで、長く話し続けたときの表示遅延を比較できる。[公式モデル資料](https://k2-fsa.github.io/sherpa/onnx/pretrained_models/offline-transducer/index.html) |
| Android `SpeechRecognizer` | 標準 API の資料では連続認識を想定していない。アプリからの音声入力は API 33 以降で、対応は認識サービスに依存する。Mote は Android 10（API 29）から対応するため、標準の認識経路からは外す。[SpeechRecognizer](https://developer.android.com/reference/android/speech/SpeechRecognizer) · [EXTRA_AUDIO_SOURCE](https://developer.android.com/reference/android/speech/RecognizerIntent#EXTRA_AUDIO_SOURCE) |

まず同じ WAV を候補に渡し、精度と発話区間を比較する。録音中の試験では `AudioRecord` を一つだけ開き、同じ PCM をアプリ専用ストレージと認識器へ渡す。モデル導入後は機内モードでも認識が成立することを確認する。Phase 3 の初回実装は端末内認識から進める。モデルの配布方法、採用モデル、データ量は端末測定後に決める。

## 日本語の使い心地と運用費を両立する構成

月 15〜40 時間を想定。まず Android だけで **Moonshine の途中表示**と **VAD（発話区間の検出）＋ ReazonSpeech k2 INT8 の発話単位表示**を比べる。VAD 構成でも、録音を続けながら話の切れ目ごとに文字を出せる。長く話し続ける場合は、最大区間を設けると待ち時間を抑えられるが、境界の重複・脱落を検証する。録音ファイルの約 5 秒の保管単位と、認識の区切りは別に扱う。

| 構成 | 候補と根拠 | Mote で見たい点 |
| --- | --- | --- |
| Android で途中表示 | **Moonshine Tiny / Small**。公式の同一評価では日本語 CER が Tiny 19.7%、Small 17.2%。**Kodama** は日本語向けの追加学習例。配布 ORT 約 320 MB、開発者の同じ24件の比較で CER は一括 10.58%、逐次 15.96%。[Moonshine の評価条件](https://github.com/moonshine-ai/moonshine/blob/main/docs/models/available-models.md) · [Kodama モデルカード](https://huggingface.co/ayousanz/kodama-ja-streaming-small) | 途中の文字が出るまでの時間と書き換わり、60 分の発熱と電池消費。Kodama は公式 Moonshine Android SDK への差し替え可否も確認する。 |
| Android で発話単位の確定 | **ReazonSpeech k2 INT8** が軽量候補。**Parakeet Japanese CTC INT8** は精度を優先する対抗候補で ONNX が約 625 MiB。両方に sherpa-onnx の Android 検証 APK がある。**SenseVoiceSmall INT8** は日英混じりの比較候補。[Android APK 一覧](https://k2-fsa.github.io/sherpa/onnx/android/apk-simulate-streaming-asr.html) · [Parakeet CTC 配布形式](https://k2-fsa.github.io/sherpa/onnx/pretrained_models/offline-ctc/nemo/japanese.html) · [SenseVoice](https://github.com/QwenAudio/SenseVoice) | 言い淀みでどこが区切られるか、話し終わってから確定するまでの時間、長い発話での待ち時間。試す量子化ファイルの版を固定する。 |
| 自宅 PC で確定・再認識 | CPU で **Whisper large-v3-turbo** の `faster-whisper` INT8、GPU がすでに使える場合は **Haas-ja-v1.0** と **Qwen3-ASR-1.7B**。Haas の著者評価は日本語5セットで重み付き CER 6.12%。公式ランタイムは CUDA を使い、モデルはオフライン認識向け。Qwen の公式ライブ経路は vLLM を使う。[faster-whisper](https://github.com/SYSTRAN/faster-whisper) · [Haas 評価](https://huggingface.co/third-intelligence/Haas-ja-v1.0#evaluation) · [Haas 実装](https://github.com/third-intelligence/Haas-ASR) · [Qwen3-ASR](https://github.com/QwenLM/Qwen3-ASR) | 手持ち PC が使えるなら月の API 従量料金は発生しない。電力と起動待ち、外出先での到達性を含める。端末に認識待ちの音声を残し、PC が使えるときに処理できる。 |
| 自宅 PC でライブ | **Nemotron 3.5 ASR Streaming 0.6B** は日本語対応のキャッシュ付きストリーミング。**Voxtral Mini 4B Realtime** も日本語対応の公開重みがあり、GPU 側の比較候補。Whisper の逐次表示を試す場合は **SimulStreaming / WhisperLiveKit** の既存実装も使える。[Nemotron](https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b) · [Voxtral](https://huggingface.co/mistralai/Voxtral-Mini-4B-Realtime-2602) · [SimulStreaming](https://github.com/ufal/SimulStreaming) | 回線切替・ロック中の切断も含めた実測。同じ日本語でも短く刻むほど遅延と精度のトレードオフが出るため、公開チャンク長を UI の遅延として扱わない。 |

読み上げの順位は用途で変わる。公開の同一評価では、Qwen3-ASR-1.7B が FLEURS の読み上げで1位、放送音声で8位だった。Android 音声入力プロジェクトの比較には、Parakeet Japanese CTC の精度が高い一方、無音に近い雑音に短い文字を出した例もある。藤江研究室の軽量な日本語ストリーミング Zipformer は対話を含む評価例があるが、重みは非公開のため、一般配布できる候補とは扱わない。[同一条件の読み上げ・放送評価](https://huggingface.co/datasets/yukobayashi500/ja-asr-leaderboard-2026) · [日本語入力の比較](https://github.com/maxwhipw/marmalade-stt-android/blob/main/docs/benchmarks/2026-10-japanese.md) · [藤江研究室の公開条件](https://github.com/fujielab/fujielab-asr#model-availability)

**Haas-ja の Android 動作例。** LiteRT のコミュニティ移植には Galaxy S26（Qualcomm SM8850）上の実測と Android 向け C++ ランナーがある。FLEURS 日本語650発話で、チップ専用 NPU 版は CER 3.70%、録音済み音声の処理が実時間の184倍、ピークメモリ約1.50 GB、モデル一式約1.35 GB。CPU INT8 版は同機種で CER 3.70%、46倍、ピーク約4.49 GB、ファイル約1.94 GB。速度はロード・ウォームアップを除く録音済み音声の通し処理。Mote ではまず停止後や発話単位の確定に使えるかを調べる。Q4_K の約352 MBの GGUF もあるが、別ランタイムであり、Android での統合・速度は未確認。[LiteRT の精度・性能](https://huggingface.co/litert-community/Haas-ja-v1.0#performance) · [GGUF の配布](https://huggingface.co/sakasegawa/Haas-ja-v1.0-GGUF)

二段構成を試す場合は、軽いモデルで途中表示し、精度の高いモデルは発話確定時か、利用者が選んだ区間だけに適用する。短文は認識後の文章をそのまま編集可能な下書きへ入れる。長文は確定区間を保存した後に別の認識結果が届いても、自動置換で利用者の修正を失わないようにする。端末内だけで足りれば従量費はなく、重い再認識だけ PC や明示的に選んだクラウドへ回せる。Google Cloud Speech-to-Text と常時従量課金のクラウドは今回の優先比較から外す。

常時使えるセルフホストが必要なら、既存の PC と固定料金の CPU VPS が候補になる。さくらの VPS の公開料金例では 2 GB で年払い月額換算 1,594 円。低価格帯の共有 CPU で 1 ストリームを維持できるかは未測定。PC の場合も、ASR の追加消費と待ち受けの電力を分けて見積もる。たとえば待ち受けのために毎日 24 時間、追加で 10 W を使うなら、30 日で 7.2 kWh になる。[さくらの VPS 料金](https://vps.sakura.ad.jp/specification/)

### 学習で改善できる範囲

1. **語彙・表記を先に比較。** `Harmion`、`Mote`、人名、製品名、略語のような間違いには、認識時の少数の用語ヒントと、確認済みの表記辞書を使う。Moonshine Streaming の keyterms、Whisper のプロンプト、ReazonSpeech を動かす sherpa-onnx の対応 transducer に用語を渡す方法がある。語を発話していない音声での誤挿入も測る。[Moonshine の用語指定](https://github.com/moonshine-ai/moonshine/blob/main/docs/models/domain-customization.md) · [faster-whisper のオプション](https://github.com/SYSTRAN/faster-whisper/blob/master/faster_whisper/transcribe.py) · [sherpa-onnx の hotwords](https://github.com/k2-fsa/sherpa-onnx/blob/master/sherpa-onnx/python/sherpa_onnx/offline_recognizer.py)
2. **発音・話し方への適応。** 語彙で改善しない場合、自分の音声と人手で確かめた正解テキストを用意する。最初は 30〜60 分を試験規模の目安にし、静かな部屋・歩行中・長い独り言を録音日やセッション単位で学習用と評価用に分ける。Moonshine の LoRA（重みの一部を少ないパラメータで調整する手法）を小規模候補とし、日本語チェックポイントと Android 用 ORT までの出力を確認する。PC 側では NeMo Parakeet と Qwen3-ASR に公開の微調整手順がある。元モデルと同じ評価音声で比較し、別の日・別の環境での悪化も確認する。[Moonshine 学習手順](https://github.com/moonshine-ai/moonshine/blob/main/docs/models/domain-customization.md#retraining) · [NeMo 学習手順](https://docs.nvidia.com/nemo/speech/nightly/asr/fine_tuning.html) · [Qwen3-ASR 学習手順](https://github.com/QwenLM/Qwen3-ASR#fine-tuning)
3. **小さいモデルを作る段階。** 既存モデルで狙った精度が出ても端末負荷が高い場合は、良いモデルの出力を人手で訂正して、小型モデルに追加学習させる。Kotoba-Whisper や Kodama が日本語向けの先例。公開コーパスを使う場合は、モデル重みと素材の利用条件を別々に確認する。日々の Capture の無断学習を前提にせず、学習用として選んだ録音だけを使う。[Kotoba-Whisper](https://huggingface.co/kotoba-tech/kotoba-whisper-v2.0) · [Kodama の学習データ](https://huggingface.co/ayousanz/kodama-ja-streaming-small) · [ReazonSpeech 利用ガイド](https://research.reazon.jp/projects/ReazonSpeech/howto.html)

実機では CER に加えて、修正して保存できるまでの時間、固有名詞の正解率、無音時の誤出力、表示済みテキストの揺れ、停止後に残る待ち時間を測る。話し始めから表示までの遅延と、Android が画面消灯中も同じ速度を維持できるかをそれぞれ記録する。

### PC での小規模比較

2026-11-26 に、FLEURS 日本語 test の公開アーカイブから先頭の異なる32文（約396秒）を選び、Ryzen 7 7735HS の CPU で実行した。16 kHz モノラルに揃え、文字を NFKC 正規化して句読点と空白を除外し、1,563文字に対する編集距離で CER を算出した。Moonshine Voice 0.1.6 には0.5秒ずつ入力し、量子化重みは `quantized_26_08_23` を使用。sherpa-onnx 2.0.3 の発話単位認識は、各ファイルを区切り済みの発話としてまとめて渡した。[評価音声と版](https://huggingface.co/datasets/google/fleurs/blob/9431b717ffeb16da9e55413b2bca3bd6aaf77e3c/data/ja_jp/audio/test.tar.gz)

| モデル | 入力方式 | モデル主ファイル | CER |
| --- | --- | ---: | ---: |
| Moonshine Japanese Tiny | ストリーミング | 約31 MiB | 12.60% |
| Moonshine Japanese Small | ストリーミング | 約116 MiB | 8.38% |
| ReazonSpeech k2 INT8 | 発話単位 | 約153 MiB | 11.13% |
| Parakeet Japanese CTC INT8 | 発話単位 | 約625 MiB | 8.25% |

Moonshine の最初の文字は、Tiny・Small とも入力済み音声の中央値2.5秒の地点で確認できた。先頭の無音を含む PCM 位置であり、端末の推論・UI を含む応答時間は実機で測る。発話単位の認識時間は入力合計396秒に対して ReazonSpeech k2 が約11秒、Parakeet が約100秒。モデル読み込み、音声デコード、発話区間の検出を除き、PC の他の負荷をそろえていない参考値。Small と Parakeet の CER の差はこのセットでは編集距離2文字分なので順位は付けない。この試験の読み上げ音声は、歩行中、言い直し、技術用語、60分連続の性能を評価していない。

## 低コストのクラウド認識

2026-11-26 時点の公開価格。金額は USD・税別。個人利用で 1 ストリームを想定し、無料枠は他の用途と共有していない前提で計算した。

| 候補 | 無料枠・単価 | Mote との相性 |
| --- | --- | --- |
| **Azure Speech F0** | **毎月 5 音声時間**、同時 1 ストリーム。 | 数回の長い録音を月内にまとめて試しやすい。東日本リージョンと日本語 `ja-JP` があり、Speech SDK に Mote の 16 kHz PCM を渡せる。F0 の上限で停止し、従量課金への切り替えは別途判断する。[料金](https://azure.microsoft.com/ja-jp/pricing/details/speech/) · [制限](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/speech-services-quotas-and-limits#real-time-speech-to-text-and-speech-translation) · [言語](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/language-support#speech-to-text) · [リージョン](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/regions) · [PCM 入力](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/how-to-use-audio-input-streams) |
| **Cloudflare Workers AI / Nova-3** | **毎日 約 11 分 57 秒**。WebSocket は 836.36 Neurons/音声分、日次枠 10,000 Neurons。超過分は有料プランで $0.0092/分、Workers Paid 自体は月 $5 から。 | 毎日の短い日本語メモと相性がよい。途中結果を取得できる。無料枠は他の Workers AI リクエストと共有し、09:00 JST に更新される。無料プランで 60 分のストリームを無料枠内に収めるには、送る音声を約 12 分以内に抑える必要がある。[モデル](https://developers.cloudflare.com/workers-ai/models/nova-3/) · [日本語](https://developers.cloudflare.com/realtime/realtimekit/ai/transcription/#real-time-supported-languages) · [Workers AI 料金](https://developers.cloudflare.com/workers-ai/platform/pricing/) · [Workers 料金](https://developers.cloudflare.com/workers/platform/pricing/) |
| **Soniox STT v6** | **約 $0.12/時間**。月 10 時間で約 $1.20、月 30 時間で約 $3.60。トークン課金のため出力で変動する。新規 API アカウント向けの無料クレジットは終了。 | 日本語対応、日本リージョンあり。1 セッション最大 300 分。ストリーム開始後は keepalive 中の無音も課金時間に入る。プリペイド残高と月間利用上限を使える。ライブ認識の音声と本文を通常は保存せず、モデル学習には使わないと明記している。[料金](https://soniox.com/pricing) · [無料クレジットの変更](https://soniox.com/blog/2025-10-27-free-credits-update-for-soniox-api) · [言語](https://docs.soniox.com/stt/concepts/supported-languages) · [日本リージョン](https://docs.soniox.com/data-residency) · [セッション上限](https://docs.soniox.com/stt/rt/limits-and-quotas) · [無音時](https://docs.soniox.com/stt/rt/connection-keepalive) · [支出上限](https://docs.soniox.com/console/limits) · [データの扱い](https://docs.soniox.com/security-and-privacy) |
| Google Cloud Speech-to-Text V1 | 毎月 60 分無料。データロギングなしの従量料金は $0.024/分（$1.44/時間）。 | 日本語 `latest_long` がある。ストリームごとに約 5 分で接続を張り直すため、長時間録音では境界での重複・欠落を検証する必要がある。[料金](https://cloud.google.com/speech-to-text/pricing) · [言語](https://docs.cloud.google.com/speech-to-text/docs/v1/speech-to-text-supported-languages) · [制限](https://docs.cloud.google.com/speech-to-text/docs/v1/quotas) |
| Speechmatics Melia 1 / Deepgram 直接契約 | Melia 1 は $0.20/時間と初回 $100 分。Deepgram Nova-3 日本語単一言語の配信は公開プロモ $0.288/時間、通常 $0.462/時間、初回 $200 分。 | 精度比較を広げるときの候補。初回クレジット消費後は従量単価で見積もる。[Speechmatics 料金](https://www.speechmatics.com/pricing) · [クレジットへの移行](https://www.speechmatics.com/company/articles-and-news/moving-to-credit-based-billing) · [言語](https://docs.speechmatics.com/speech-to-text/languages) · [Deepgram 料金](https://deepgram.com/pricing) · [言語](https://developers.deepgram.com/docs/models-languages-overview) |

**録音後の再認識**には、Cloudflare Workers AI の Whisper large-v3-turbo が 46.63 Neurons/分で約 214 分/日、従量単価は $0.03/時間相当。Groq の同モデルは公開無料上限が 2 音声時間/時・8 音声時間/日、従量単価は $0.04/時間で、課金時は 1 ファイル最低 10 秒分。両者は音声ファイルを渡す API。Cloudflare でライブ認識と併用する場合は日次枠を共有する。Groq は通常のデータ保持条件と Zero Data Retention の設定を検証前に確認する。[Cloudflare Whisper](https://developers.cloudflare.com/workers-ai/models/whisper-large-v3-turbo/) · [Cloudflare 料金](https://developers.cloudflare.com/workers-ai/platform/pricing/) · [Groq 無料上限](https://console.groq.com/docs/rate-limits) · [Groq 音声 API](https://console.groq.com/docs/speech-to-text) · [Groq データの扱い](https://console.groq.com/docs/your-data)

月 15〜40 音声時間の想定なら、30 日に分散した場合は 1 日平均 30〜80 分で、Whisper の両候補とも公開無料枠に収まる。まとめて処理する場合は月合計より時間・日単位の上限が先に効く。Groq の全量を従量単価で処理した場合は月 $0.60〜$1.60 が目安。Soniox は同じ時間数でファイル認識が月約 $1.50〜$4.00、ライブが月約 $1.80〜$4.80。ライブは接続時間、ファイル認識は送る音声時間をもとにした試算。録音後の認識には端末に保存した断片を再送できる利点があり、録音中の途中表示はライブ認識か端末内認識で評価する。[Groq の料金](https://console.groq.com/docs/model/whisper-large-v3-turbo) · [Soniox の料金](https://soniox.com/pricing)

クラウドの試験は Azure F0 を無料の比較基準、Soniox を長時間の低価格比較候補とする。Cloudflare は短時間のライブ入力と録音後の再認識で比較する。同じ日本語音声に無音区間と通信断を加え、途中・確定結果の遅延、再接続時の重複、送信音声秒数、接続時間、実際の使用量を確認する。無音の音声フレームを送り続ける場合は、発話時間だけで費用を見積もらない。

クラウド送信は録音ごとの明示操作で有効にし、送信先を表示する。無料枠・予算の上限や通信断が来たらクラウド送信を止め、端末の録音を残す。長期キーは APK に含めず、登録済みサービスから短命の認証情報を取得する方式を検証する。Azure の短命トークンは 10 分で、長い録音では更新の確認が必要。Soniox は一時キーを使える。[復旧設計 §3](mote-recovery.md#3-録音の寿命と復旧できる範囲) · [Azure の認証](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/troubleshooting#validate-an-authorization-token) · [Soniox の一時キー](https://docs.soniox.com/authentication/temporary-api-keys)

## 次の実装で固定する境界

- 可視画面の明示操作で microphone 型 Foreground Service を開始する。録音中は通知に停止操作を出す。Android 13 以降では通知権限の拒否時に Foreground Service を起動できる場合でも、通知欄の停止操作を提供できないため Mote は開始前に通知設定を確認する。[マイク型サービス](https://developer.android.com/develop/background-work/services/fgs/service-types#microphone) · [通知権限](https://developer.android.com/develop/ui/compose/notifications/notification-permission)
- 途中結果は画面に表示し、確定区間のみ録音 ID とサンプル範囲で永続化する。無音区間では発話を区切っても録音を続ける。録音停止時は最後のサンプルまで確定できた区間と未認識区間を区別する。
- 短文は停止後に編集できる下書きへ取り込み、既存の送信で Content を 1 件作る。長文は確定断片を端末へ先に保存し、同じ Content へ入力順にまとめて追記する。既存の長文ジャーナルは保留 1 件を扱うため、音声の確定処理を Activity の下書き更新に流さず、連番付きの永続キューと保存処理の直列化を先に実装する。Harmion 側の書き込み結果が不明な場合は、既存の照合手順で確認できるまで後続の追記を止める。
- 長文の停止は次の入力ができる状態に戻す。完了は録音停止と最後の認識・追記の照合まで待つ。Harmion 側の追記は全文 edit のため、確定断片のローカル保存とノードへの送信の頻度を分ける。

## デバッグ版の録音検証

```sh
nix develop .#android
gradle -p android :mote:assembleDebug :mote:testDebugUnitTest
adb install -r android/mote/build/outputs/apk/debug/mote-debug.apk
adb shell am start -n dev.satler.mote/org.harmion.mote.voice.VoiceProbeActivity
```

既存の Mote に上書きする場合は、既存 APK と同じ署名を使う。

「開始」を押し、初回は権限を許可してもう一度「開始」を押す。表示される録音 ID と保存秒数をメモする。画面を離れて再度開き、同じ ID と進んだ保存秒数を確認する。通知から「停止」を押すと、最後の分割片を書き込んだ後に通知が消える。アプリを強制停止した場合は、その時点でファイル化されていない末尾が失われる。再起動時に録音は再開しない。

音声は `no_backup/voice-probe/` に 16 kHz、モノラル、16-bit PCM の WAV として約 5 秒ごとに置く。ファイル名の末尾は開始サンプル。異常終了で残った `.part` は検証データとして保留し、完成 WAV の集計と区別する。1 回 60 分、保存先全体 256 MiB は検証用の上限。WAV は約 115 MB/時で、製品版の圧縮形式・保持日数・容量上限は実測と復旧要件に基づいて決める。録音検証には自動削除がないため、試験後は録音を停止してから、確認した録音 ID のファイルだけを削除する。

```sh
adb shell run-as dev.satler.mote ls no_backup/voice-probe
# 一覧にある WAV ファイル名を指定して確認用に取り出す
adb exec-out run-as dev.satler.mote cat no_backup/voice-probe/FILE.wav > FILE.wav
```

Android の別ユーザーで試す場合は、`run-as dev.satler.mote --user USER_ID` を使う。

2026-11-26 のエミュレーター試験では、ホスト音声を無効にし、ホームへ戻って画面を消灯している間も WAV が増えることを確認した。通知から停止した後、約 21 秒分の WAV 5 件を読み出し、形式と開始サンプルの連続性を確認した。

## 実機での判定

1. **録音経路。** 利用予定の Android 端末で 10 分間の試験を行い、画面回転、他アプリへの移動、ロックと復帰、通知停止を試す。WAV のサンプル範囲に加え、ロック中も一定間隔で流したテスト音が途切れずに入るかを確認する。停止後に通知が消え、権限拒否、強制停止、容量上限でも既存 WAV が残ることを確かめる。
2. **日本語認識。** 同一の日本語メモ音声で、固有名詞、英数字、長い無音、言い直し、長い連続発話を比較する。文字誤り率（CER）と、発話開始から最初の途中結果、発話終了から確定までの時間を測る。暫定目安は 95 パーセンタイルでそれぞれ 2 秒、3 秒。途中・確定の重複、欠落、無音での誤認識を記録する。
3. **長時間。** 画面消灯を含む 60 分間で、認識の遅れ、メモリ、温度、バッテリー消費、WAV 容量を記録する。録音単独と録音＋認識を比較する。長文統合後は通信断と復帰を含め、キューの残り、Harmion への送信量、最終本文への反映まで計測する。
