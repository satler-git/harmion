# Mote デスクトップ HUD 実装計画

導入手順とキー一覧は [mote-hud-install.md](hud-install.md)。

2026-11-28 / 状態: 計画（主要な決定済み）/ 対象: Linux (niri + Wayland)

[UX 草案](desktop.md) の v2 Quick Panel を、既存の CLI と `mote-core` の上に実装する。画面の正は OpenPencil の `mote-desktop`（Page 2 `v2 — Quick Panel`）とし、v1 HUD の広い2ペインは今回の対象から外す。

## 1. 要約

- 描画は **AntiWebUI の layer surface**。`mote-hud` という常駐プロセスが、召喚のたびに 1 枚の surface を開く。
- 召喚はコンポジタのキーバインドから `mote-hud toggle`。Unix socket で常駐プロセスに伝えるため、2回目以降は待ちなしで開く。
- データは CLI と同じ capture-only credential と `harmion-client` を直接使う。CLI を subprocess で叩かない。
- `mote-core` は純粋なまま保ち、認証・outbox・ノード読み書きを `apps/mote` の lib に寄せて CLI と HUD で共有する。
- 一覧 640×680、詳細 720×800。Capture・処理・コマンドは同じ surface の中のシート。
- 実装は P0（spike）→ P1（共有データ層）→ P2（一覧）→ P3（詳細・処理）→ P4（Capture・outbox）→ P5（仕上げ）。
- v2 デザインにある「次にやる / Wikiへ / タスクへ」と長文の追記は、現行のデータモデルに無いため V1 から外す（§8）。

## 2. 前提の確認

| 項目 | 確認結果 |
| --- | --- |
| 画面 | Pinned の AntiWebUI に `zwlr_layer_shell_v1`、`zwp_text_input_v3`（IME）、`virtual_list`、クリップボード、影、`on_scrolled` がある |
| 合成 | niri 26.04、eDP-1 1723×1076（scale 1.3）。640×680 と 720×800 は余白込みで収まる |
| フォント | IBM Plex Sans JP / JetBrains Mono が導入済み |
| 既存 CLI | `q` / `list` / `show` / `search` / `archive` / `pin` / `pending` / `retry` / `forget` / `connect` / `auth`。編集トークン付きの read view を取得できる |
| 未実装 | CLI に削除・長文の追記が無い。outbox にプロセス間ロックが無い |
| デザイン | 一覧 640×680、詳細 720×800、Capture 600×420、処理 600×360、コマンド 560×520 |

## 3. アーキテクチャ

### 3.1 プロセスと surface

```
niri keybind ──spawn──> mote-hud toggle ──socket──> mote-hud daemon
                                                      │
                                                      ├─ Shell::run + IPC listener
                                                      ├─ Session actor (tokio)
                                                      └─ Layer surface (開いている間だけ)
```

- `mote-hud toggle` は socket に繋がなければ自分を `--daemon` で起動し、2秒待って再接続する。起動側は stdio を null にしてすぐ終了し、デーモンは孤児として残す。
- socket は `$XDG_RUNTIME_DIR/mote-hud.sock`（0600）。`XDG_RUNTIME_DIR` は 0700 なので、同じユーザー以外は接続できない。
- 受信スレッドから直接 `open_layer` は呼べない（AntiWebUI のウィンドウ操作は UI スレッドの thread-local）。`ui_channel` の送信側を listener に渡し、UI スレッドのコールバックで surface を開閉する。
- surface は **Overlay / 中央 / 固定サイズ / keyboard Exclusive / exclusive Ignore / namespace `mote-hud`**。
- 背景は透明。角丸の外と影の範囲は入力を受けず、パネルの矩形だけ `InputRegion::Rects` にする。
- `Esc` はシート → 詳細 → surface を順に閉じる。最後に閉じたときにコンポジタが元のウィンドウへフォーカスを返すことを P0 で確認する。
- フォーカスを失ったら閉じる（`Focused(false)`）。キーボード Exclusive のまま他のウィンドウをクリックした時に HUD が残らないようにする。P0 でイベントが来ない場合は `OnDemand` か、出力全体の透明な入力面に切り替える。
- 表示先は niri IPC の focused output。取れない場合はプライマリ。
- 最後の surface を閉じてもデーモンは残る。30分使われなければ終了し、次の召喚でキャッシュから即復帰する。

**常駐にする理由。** niri はアプリ内グローバルホットキーを持てないため、召喚のたびにプロセスを起動すると wgpu の初期化を毎回払う。デーモンはデータと surface の生成コストだけを払う。メモリは P0 で実測し、200MB を超えるなら起動のたびに終了する方式へ切り替える。

### 3.2 クレート構成

```
crates/mote-core         既存のまま（純粋モデル）
apps/mote                lib + CLI
  src/lib.rs             auth / spool / client / error を公開
  src/main.rs            既存 CLI（lib を使う）
apps/mote-hud            新規
  src/main.rs            toggle / open / capture / daemon / snapshot
  src/ipc.rs             socket とコマンド
  src/session.rs         Session actor、購読、楽観更新
  src/cache.rs           snapshot と view state
  src/surface.rs         surface の開閉とサイズ
  src/ui/                theme / panel / row / detail / sheets / icons
```

`mote-hud` だけが wgpu / AntiWebUI に依存する。CLI の依存は増やさない。`apps/mote` を lib 化するとき、CLI の引数解釈と表示は `main.rs` に残し、ノードアクセスと保存要求だけを `client.rs` に移す。

### 3.3 データフロー

- Daemon 起動時に `~/.local/state/mote/snapshot.json` を読み、接続前に一覧を出す。snapshot には edit token を保存しない（復帰後は再取得まで読み取り専用）。
- デーモン起動後に capture 全体を購読し、切れたら再購読する。購読が使えない時は、surface を開いた時と操作の後に全件 query する。フィルタと検索は手元のスナップショットで行う。
- 1つの `Session actor` がノードとキャッシュを所有し、`Arc<Model>` を `SharedSignal` で UI に渡す。UI はコマンドを channel で送る。
- 書き込みは楽観更新。ローカルに適用して表示し、保存が失敗したら戻して status に出す。アーカイブとピンは取り消せる。
- outbox は CLI と同じ `~/.local/state/mote/outbox`。Capture は spool に書いてから送る。成功したら spool を消し、同じ content ID の行をノードの Capture に置き換える。
- credential は期限前に更新する。ファイルロックと「更新前に読み直す」処理を追加し、CLI と HUD が同時に refresh して片方が refresh token を失う事故を防ぐ。

### 3.4 永続化

```
$XDG_STATE_HOME/mote/
  snapshot.json      captures（edit token 抜き）、last_sync、saved_at
  view.json          最後のフィルタ、選択 ID、スクロール位置、last_seen
  outbox/            既存の PendingCapture
  *.lock             outbox と credential の排他
```

`MOTE_STATE_DIR` を尊重する。snapshot と view は 0600、ディレクトリは 0700。書き込みは temp + rename。

## 4. 画面

### 4.1 一覧（640×680）

```
Header 64       Mark · Search 40（/ ヒント） · Filter chip · +
List 520        日付ヘッダ 32 / 選択行 78 / 通常行 44–60 / 前回ここまで
ActionBar 56    処理 · アーカイブ · … ·                     開く
Status 40       ● 接続済み 12:31  未送信 1      j/k 移動  Ctrl+K アクション  ?
```

- 日付ヘッダは `11月27日（金）`、今日なら `今日 8件`、右に `新着 3`。sticky は V1 では行わない（後に追加）。
- 行は左から 2px のアクセント（選択時）、固定幅の時刻（10.5 mono）、本文（13.5/19、最大3行）、末尾の字数・返信・ピン・未送信。
- 行の高さは `max(44, 21 + 19 × 行数)`。行数は本文の文字幅から決定的に見積もる（測ってから変わらないのでスクロールが飛ばない）。
- `前回ここまで` は、最後に閉じた時刻より古い最初の行の前に引く。
- 未送信は Amber の点、ピンはブックマーク、返信は曲がった矢印。色だけで区別しない。
- 検索は手元で即時。一致箇所の前後を出し、該当語をアクセントで強調する。
- `○字` は3行に収まらず切り詰めた時だけ出す。数字は本文全体の文字数（空白・改行を含む）で、2行に収まる短文や、長文でも全文が収まる場合は出さない。
- マウスでも動く。行のクリックで選択、ダブルクリックで詳細、ボタンとチップはそのまま押せる。キーボードが主で、マウスは補助。
- 空・読込中・オフラインを区別する。空の Inbox と「まだ読めていない」を同じ見た目にしない。
- 召喚時は一覧から開く。前回のフィルタ、選択、スクロールだけ戻し、詳細やシートは開き直さない。

### 4.2 詳細（720×800）

- 戻る（Inbox）、日付、時刻 20 mono、字数と長文、本文 15/26、フッターに capture / short id / 更新時刻。
- 長文セッションのバナーは `session_open` がある時だけ出す。追記と完了は P4 の任意項目（§8）。
- アクションは アーカイブ / ピン / コピー / …。返信はコマンドメニューから。

### 4.3 シート

| シート | サイズ | 内容 |
| --- | --- | --- |
| Capture | 600×420 | 複数行入力、`Ctrl+Enter` 保存、`Esc` 閉じる、任意で長文トグル |
| 処理 | 600×360 | 完了（アーカイブ）・コピー。行は意味のあるものだけ出す |
| コマンド | 560×520 | 検索、この項目、移動。行は意味のあるものだけ出す |
| ヘルプ | 560×520 | キー一覧。読み取り専用 |
| 削除確認 | 420×180 | 削除とキャンセル。`d` の後にだけ出す |

シートは一覧の上に scrim と影付きで重ねる。幅はパネル内に収め、surface は広げない。詳細だけは 720×800 に広げ、戻ると 640×680 に縮める。

```
List ──Enter──> Detail ──Esc──> List
 ├─ /    検索（同じ一覧のまま）
 ├─ i/+  Capture ──Ctrl+Enter──> List
 ├─ Ctrl+K Command ──Enter──> 実行して List
 ├─ m    処理 ──Enter──> 実行して List
 └─ ?    ヘルプ ──Esc──> List
```

`Esc` は開いているシートを1枚だけ閉じる。シートが無ければ詳細を閉じ、詳細も無ければ surface を閉じる。削除確認だけは `Enter` では実行せず、明示的に選ばせる。

### 4.4 キー

| キー | 動作 |
| --- | --- |
| `Mod+S` | HUD を一覧で開閉（コンポジタ） |
| `Mod+Shift+S` | Capture で開く（コンポジタ） |
| `j` / `k` / `↓` / `↑` | 移動 |
| `Enter` | 詳細（行が無ければ何もしない） |
| `Esc` | シート → 詳細 → 閉じる。検索中は検索解除 |
| `/` | 検索にフォーカス |
| `Ctrl+K` | コマンドメニュー |
| `a` / `p` / `c` / `r` / `d` | アーカイブ / ピン / コピー / 返信 / 削除 |
| `m` | 処理シート |
| `i` または `+` | Capture シート |
| `?` | キー一覧 |
| `Ctrl+Z` | 直前のアーカイブ・ピンを取り消す |
| `Ctrl+Enter` | Capture を保存 |

- 単キーは一覧にフォーカスがある時だけ。検索・Capture・IME 変換中は奪わない。
- Capture は `Enter` を改行に残す（日本語 IME の確定と衝突させない）。
- コマンドメニューのショートカット表示はデザインの `Ctrl+Shift+A` ではなく、実際に効く `a` / `p` / `c` / `r` / `d` にする。

niri の `binds` に追加する。

```kdl
Mod+S { spawn "mote-hud" "toggle"; }
Mod+Shift+S { spawn "mote-hud" "capture"; }
```

### 4.5 見た目

`.fig` から値をそのまま `ui/theme.rs` に置く。

| token | 値 | 用途 |
| --- | --- | --- |
| panel | `#17141F` | パネル |
| raised | `#191520` / `#1D1927` | アクションバー / 入力欄 |
| selected | `#2A2338` | 選択行 |
| border | `#2A2436` | 枠、区切り |
| text | `#FAF6FF` | 本文 |
| text-secondary | `#C9BEDB` | 時刻、ボタン |
| text-muted | `#8F84A1` | 補助、字数、ヒント |
| accent | `#D9C0FF` | 選択バー、開く、フォーカス |
| on-accent | `#241A33` | アクセント上の文字 |
| warning | `#FFCF70` | 未送信、要確認 |
| success | `#5BE38A` | 接続済み |

角丸はパネル 18、入力 9、行 8、ボタン 7。影は `0 24 64` を基準に、surface の透明余白（96px）内で切れないように調整する。動きは開閉のフェード 100ms だけにし、行の移動や選択にアニメーションを付けない。

## 5. 実装順

### 状態

| Phase | 状態 | 備考 |
| --- | --- | --- |
| P0 | 完了（IME の実機確認だけ残る） | 2026-11-28。layer surface、召喚、Esc、フォーカス復帰を niri で確認 |
| P1 | 完了 | lib 化、credential / outbox のロック、Session actor、snapshot、実ノード接続まで |
| P2 | 一部完了 | 実データ、検索、選択、スクロール、日付ヘッダ。仮想化・前回ライン・フィルタ・sticky は次 |
| P3 | 完了 | 詳細、ピン、コピー、アーカイブ、削除（確認）、返信、処理シート、コマンドメニュー、ヘルプ |
| P4 | 完了 | Capture、返信、spool 先行保存、非同期送信、未送信表示、オフラインの cached list と pending を確認 |
| P5 | 完了 | 仮想化、全操作のキー、Nix パッケージ、動的インストール、niri キー、テスト、IME の text-input 確認 |

### P0 — surface spike（S）

- `apps/mote-hud` の骨組み。`open_layer` で 640×680 の中央パネル、keyboard Exclusive、角丸、影、透明余白。
- niri のキーバインドから `toggle`。socket とデーモン、2回目の召喚。
- 詳細サイズへの `set_size`、Esc で閉じてフォーカスが戻ること、フォーカス喪失で閉じること。
- 実際の画面で IME（日本語変換）とクリック位置、scale 1.3 の描画を確認する。初回起動と2回目以降の召喚にかかる時間、アイドル時の RSS を記録する。

**完了条件:** キー1つで開く（初回はデーモン起動込み、2回目以降は体感で即時）。日本語が入力でき、Esc で元のウィンドウに戻る。layer surface に問題があれば、ここで niri の floating window 方式に切り替える判断をする。

**確認できたこと。**

- `zwlr_layer_shell_v1` の Overlay layer に 640×680 のパネルを出せた。透明な余白、角丸、影、scale 1.3 の位置とサイズは意図通り。
- `Keyboard::Exclusive` で niri がキーボードを渡す。ただし **root を明示的に `focus()` しないとキーが届かない**。`focusable()` だけでは足りなかった。開くたびに root を focus する。
- Esc で閉じ、閉じた後に元のウィンドウ（Firefox）へフォーカスが戻ることを `niri msg focused-window` で確認。
- socket の `open` / `close` / `toggle` で開閉できる。パネルを閉じてもデーモンは残る。
- Capture の複数行入力に文字を入力できた。`wtype` では IME の変換前表示までは再現できないので、fcitx5 / SKK での変換確定は手動で確認する。
- 開発ビルドは Wayland の `dlopen` のため、`LD_LIBRARY_PATH=$(pkg-config --variable=libdir wayland-client)` が必要。nix パッケージでは wrapper で吸収する。
- フォーカス喪失で閉じる処理は、AntiWebUI がウィンドウのフォーカス変化を公開していないため P0 では入れていない。必要なら P3 で、出力全体の透明な scrim か `OnDemand` に切り替えて実現する。
- テスト用に `MOTE_HUD_SOCKET` で socket の場所を差し替えられる。通常は `$XDG_RUNTIME_DIR/mote-hud.sock`。

### P1 — 共有データ層（M）

- `apps/mote` を lib + bin にし、credential と outbox を lib から使えるようにした。`Error` / `state_dir` / `now_micros` も lib に移した。CLI のテストは通っている。
- credential の refresh に排他ロックと「更新前の読み直し」を追加した。負けた側が死んだ refresh token を保存しない。
- outbox の `retry` / `forget` にロックを追加した。CLI と HUD が同じ保留を同時に送らない。
- `apps/mote-hud/src/session.rs` を追加した。credential を読み、ノードを query し、outbox を読み、`SharedSignal<Arc<Model>>` で UI に渡す。snapshot は state dir に保存し、edit token は保存しない。
- `--snapshot` のヘッドレス描画は P0 で追加済み。

**完了:** Session を surface に接続し、実ノードの capture を一覧に出した。ノード停止時は snapshot を読み取り専用で出す。

**確認できたこと:** サンドボックスではノードディレクトリが読み取り専用なので、`target/node-main` に写したノードで実データを表示した。`mote list --json` と同じ capture が出ること、ピンがノードに反映されること（直後に unpin で戻した）を確認した。

### P2 — 一覧（L）

- theme / 行 / 日付ヘッダ / 前回ここまで / アクションバー / ステータス。
- 可変高さの仮想リスト、選択とスクロールの復元、検索、フィルタ。
- 空・読込中・切断・要確認の状態。

**完了条件:** 1,000件でスクロールと検索が実用速度。召喚時に前回の位置が戻る。デザインとの差分を snapshot で確認できる。

### P3 — 詳細と処理（L）

- 詳細（720×800）、引用、フッター、戻る。
- アーカイブ / ピン / コピー / 返信 / 削除（確認つき）。楽観更新と取り消し。
- コマンドメニュー、処理シート、ヘルプ、フィルタメニュー。
- 競合・削除競合のある項目は操作を無効にし、状態を表示する。

**完了条件:** キーボードだけで一覧から処理まで完結する。`a` の連続処理で選択が次へ移り、`Ctrl+Z` で戻せる。日本語 IME の確定で送信されない。

**P2 で確認できたこと。**

- 実ノードの 39 件（Inbox 16 件）を新しい順に出し、今日と前日の日付ヘッダで分けた。
- `j` / `k` / `↑` / `↓` で選択が動き、選択行がアクセント付きで見える。
- `/` で検索にフォーカスし、`Mote` で 4 件に絞れた。`Esc` で検索を解除して一覧に戻る。
- 一覧は 520px でスクロールし、選択が画面外に出た時だけ最小限スクロールする。
- `1` / `2` / `3` / `4` と `f` で Inbox / All / Pinned / Archived を切り替える。Archived ではアーカイブ済みを日付順に出す。
- 前回開いた時刻を `hud-view.json` に保存し、それより古い最初の行の前に「前回ここまで」を引く。その日の中で前回以降の件数を「新着」に出す。
- シートの scrim はパネルの角丸内にクリップする。透明な影の余白まで暗くしない。
- ヘッダのフィルタと検索の高さを 40px に揃え、フィルタの幅を固定して検索欄の幅を動かさない。H/L は下のヒントに出す。
- 削除確認は既定がキャンセル。Tab / ←→ で選択を切り替え、Enter は選択中のボタンだけを実行する。
- Capture の外部シートは一覧キーを一切処理しない。入力中の `a` / `d` で裏の一覧を変更しない。
- 長い詳細は `j` / `k` / PageUp / PageDown / Space でスクロールする。
- 行クリックで選択、選択中の行をもう一度クリックで詳細。
- アーカイブ済みは All でアーカイブアイコンと薄い本文、未送信は「未送信」ラベル。
- 起動時に snapshot を読み、ノードの応答後に置き換える。クエリは 32 件ずつページングする（64 はノードが 400 を返す）。

**P3 で確認できたこと。**

- 選択行で `l` / `Enter` を押すと詳細が開き、日付・時刻・字数・本文・来歴を出した。詳細は一覧と同じ 640×680 で、開閉時に layer surface をリサイズしない。
- `h` / `Esc` で一覧に戻り、選択位置が残った。
- 詳細で `p` を押すとピンがノードに保存され、一覧の行にブックマークが出た。`c` で本文だけがクリップボードに入り、HUD を閉じた後も `wl-copy` が保持する。
- 返信は一覧にも「返信 · 親の時刻 · 親の本文」を1行で出し、詳細では本文の上に親の引用ブロックを出す。
- `a` はアーカイブして一覧に戻る。長文セッション中は拒否する。

**コマンドメニューと Capture で確認できたこと。**

- `Ctrl+K` または `m` で 560×520 のメニューが開き、検索欄にフォーカスする。`開く / アーカイブ / ピン / コピー / 削除 / 閉じる` を矢印と Enter で選べる。
- `i` で HUD 内に Capture シートを開き、`?` でキー一覧を開く。返信は `r`。マウス専用の操作はない（ボタンにもキーがある）。
- `d` で 420×180 の削除確認を出し、Enter で削除、Esc で戻る。テスト用に作った Capture を実際に削除して、ノードから消えることを確認した。
- Capture シートで本文を入力し、`Ctrl+Enter` で spool に保存して閉じ、非同期送信でノードに 1 件作られることを確認した。その後、削除の確認シートから同じ Capture を消した。
- Capture は spool に先に書く。送信は Session の別コマンドで行うため、ノードが落ちていても本文は残る。

### P4 — Capture と outbox（M）

- Capture シート、spool 先行保存、オフラインでも閉じる。
- 未送信行、`retry` / `forget`、送信成功時の置き換え。
- 長文セッションは、データ層に作成と追記を足せる場合だけ実装する（§8）。

**完了条件:** ノードが落ちていても書いて閉じられ、復帰後に送れる。二重送信しない。

**オフラインと返信で確認できたこと。**

- 到達できない endpoint を持つノードコピーで起動し、snapshot の一覧と「前回の一覧 13:16」を表示した。
- その状態で Capture を保存すると spool に残り、一覧に amber dot の未送信行、「未送信 1」、日付ヘッダの「新着 1」が出た。再起動しても残る。
- 返信は選択行の `r` から本文を書き、`Ctrl+Enter` で親 ID 付きの Capture としてノードに保存された。確認後に HUD の削除で消した。
- ダイアログを閉じたあとに root へフォーカスを戻すようにした。これがないと、返信の保存後に `d` などが効かなかった。

**P5 で確認できたこと。**

- 一覧はスクロール位置の前後 200px だけを描画する。1,000 件でも行の Div は可視分だけ作り、日付ごとの件数は事前計算する。
- スクロール容器を dyn_view の外に固定した。中に入れると、再描画のたびにスクロール位置が 0 に戻り、選択行を見失った。
- 検索とフィルタを変えたらスクロールを先頭に戻す。
- 処理シート（`m`）に 完了 / コピー / 返信 / 閉じる。コマンドメニュー（`Ctrl+K`）と役割を分けた。
- ダイアログを閉じたら root にフォーカスを戻す。返信の保存後にキーが効かなくなったため。
- `nix build .#mote-hud` が通り、`result/bin/mote-hud` が単体で起動した。wrapper の Wayland / Vulkan / フォント設定も効いている。release ビルドの 1,000 件描画は 555ms。
- 他の Harmion アプリと同じ動的インストールにした。`~/.local/bin/mote-hud` から layer を開き、close / quit まで確認した。導入手順は [mote-hud-install.md](hud-install.md)。
- `cargo test -p mote-hud` は 5 件、`cargo test -p mote` は 13 件通過。
- `--bench-list` で 100 / 500 / 1,000 件を描画した。headless の初期化込みでいずれも約 750ms で、件数に比例しなかった（1000 件: 756ms、100 件: 754ms）。行の Div は可視分だけ作れている。Nix の release ビルドでは 1,000 件が 555ms。

**ユーザーレビュー後の調整。**

- 一覧の日付横の件数、返信の親時刻、正常時の接続テキストと同期時刻を消した。返信は親本文の1行だけを返信アイコン付きで出す（右側の返信アイコンは削除）。
- `J` / `K` で5件ずつ移動。詳細では5倍量をスクロール。
- 選択は capture ID（未送信は spool の識別子）で保持し、新着が上に入っても同じ項目を選び続ける。
- コマンドメニューは検索入力とショートカットが衝突するため、キー列を消した。処理シートは `a` / `c` / `r` / `Esc` を実装。どちらの「閉じる」もシートだけを閉じる。
- 正常時は緑ドットだけ。異常時だけ状態・エラー・未送信件数を出す。
- フィルタと検索の高さを 40px に揃え、フィルタ幅を固定。H/L は下のヒントへ移した。
- `l` / `h` で詳細を開閉。詳細は一覧と同じサイズで、リサイズしない。
- コピーは本文のみ。`wl-copy` で HUD を閉じた後も残る。
- Capture の外部シートは一覧キーを処理しない。削除確認は既定キャンセル。
- ヘルプは2列。長い詳細は `j/k` / PageUp/PageDown / Space でスクロール。行クリックで選択、再クリックで詳細。

**P5 で残っていること。**

- IME の配管は確認できた。`WAYLAND_DEBUG=1` で Capture の入力欄にフォーカスすると `zwp_text_input_v3.get_text_input`、`enter`、`enable`、`set_cursor_rectangle`、`commit`、`done` が流れる。text-input v3 は有効。
- ただし `wtype` の仮想キーボードは fcitx5 を通らないため、preedit の見た目は実キーボードで確認する必要がある。テスト後、fcitx5 は元の `keyboard-jp` / inactive に戻した。
- niri のキーバインドはユーザーが dotfiles に追加済み。`niri msg action spawn -- mote-hud toggle` で layer が開くことを確認した。`wtype` の仮想キーボードでは niri のバインドが発火しないため、実際の `Mod+S` は物理キーでの確認が必要。
- 実キーボードでの preedit の目視だけが残っている。

### P5 — 仕上げ（M）

- キー一覧、アクセシビリティラベル、フォーカス表示、モーション。
- `mote hud` から `mote-hud` を起動する薄い入口。
- nix の `mote-hud` パッケージと niri 設定の例。
- パフォーマンス（開くまで、1,000件、メモリ）と受入チェック。

## 6. 検証

- `--snapshot` で一覧・詳細・Capture・コマンド・処理・空・切断を PNG にし、`docs/mote-v2-*.png` と並べて目視差分を取る。1x と 2x の両方。
- `mote-hud` の単体テスト: 行高の見積もり、日付グルーピング、前回ライン、選択移動、検索、undo。
- actor のテスト: 楽観更新の巻き戻し、outbox の置き換え、refresh の読み直し。
- IPC のテスト: stale socket、同時 toggle、デーモン不在時の起動待ち。
- 手動: niri で召喚 → 日本語入力 → 詳細 → 処理 → 取り消し → 切断中の表示 → 復帰。マルチモニタは P5。

## 7. リスク

| リスク | 対策 |
| --- | --- |
| layer surface の keyboard Exclusive と IME が niri で期待通りに動かない | P0 で最初に確認。駄目なら niri の floating window + `focus-window` IPC に切り替える |
| 影が surface の端で切れる | 透明余白を 96px 取り、入力をパネル矩形に絞る。切り取られるなら影を弱める |
| 可変高さの仮想リストでスクロールが飛ぶ | 行高を測らず決定的に決める。P2 の受入で 1,000件を確認 |
| CLI の lib 化で既存の挙動を壊す | 移動だけに留め、既存テストと CLI の出力を変えない。P1 で `mote list --json` の差分を取る |
| CLI と HUD の同時 refresh で refresh token を失う | ファイルロック + 読み直し + refresh 失敗時の再試行 |
| 常駐でメモリを使い続ける | P0 で実測。超えるならアイドル終了を短くする |
| nix の `mote-hud` パッケージで wgpu / Wayland の依存が足りない | 解消。`nix build .#mote-hud` と単体起動を確認済み |
| offline 検証でノードごとコピーすると refresh token がローテートして本体の認証が切れる | 実際に発生。`mote connect` で再認証した。以後は credentials をコピーせず、切断表示だけなら snapshot と到達不能 endpoint で行う |
| デザインと実装の文字組みがずれる | フォントは導入済み。13.5/19 と 10.5 をそのまま使い、snapshot で比較する |

## 8. 決めたこと

2026-11-28 に確認済み。実装はこの決定で進める。

| # | 論点 | 決定 |
| --- | --- | --- |
| 1 | 常駐デーモンか、召喚ごとの起動か | 常駐 + 30分アイドル終了。P0 のメモリ実測で再判断 |
| 2 | 詳細を 720×800 に広げるか、640×680 の中で見るか | デザイン通り広げる。wide（v1）は作らない |
| 3 | 「次にやる / Wikiへ / タスクへ」 | V1 では出さない。Mote の属性に無く、要件の「整理しない」と衝突する。行き先の Kind が決まってから足す |
| 4 | 長文の追記・完了 | 詳細は読み取り専用で出す。データ層に追記を足すなら P4、足さないなら V1.1 |
| 5 | 並び順 | デザイン通り新しい順。Android の古い順とは変える |
| 6 | 削除 | 確認つきで入れる。プロトコルは対応済み。認可で拒否されたらアーカイブへ倒す |
| 7 | 複数選択とトリアージモード（v1 の `x` / `t`） | V1 では作らない。一覧と詳細で足りるかを見てから |
| 8 | 召喚キー | `Mod+S` で一覧、`Mod+Shift+S` で Capture。niri 設定ではこの2つを追加する。`Mod+Shift+C` は center-column が使用中 |
| 9 | 召喚直後のフォーカス | 一覧に置く（`j/k` ですぐ処理できる）。検索は `/` かクリック。v2 の見た目に合わせて検索欄に置くなら、単キー操作の入口を別に決める |

## 9. この計画で作らないもの

音声入力・ASR、本文の編集（長文の追記を除く）、タグ・フォルダ・階層、Wiki への自動搬出、独自同期、トレイ常駐、アプリ内グローバルホットキー。
