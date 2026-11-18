# Update record v1

更新レコードの保存・Peer中継用形式。App/Provider APIのJSON DTOとは独立。以下の形式とfixtureを互換性の基準とする。未対応version・タグ・非canonical入力を拒否し、受信側で別のbytesへ修正しない。

## 共通符号化

- 多バイト整数はbig-endian。長さ・個数はu32、時刻はi64（2の補数、UTCマイクロ秒）。
- IDは32 bytes。codecは発行せず、暗号学的乱数生成はhostが行う。
- 文字列はu32のUTF-8 byte長＋そのbytes。不正UTF-8は拒否し、Unicode正規化はしない。
- ID集合はu32個数＋IDのbyte辞書順（厳密昇順）。重複を拒否。
- 属性オブジェクトはキーのUTF-8 byte辞書順。配列は入力順・重複を保持。
- レコードは最大1,048,576 bytes。属性値の根の深さを0とし、最大64。
- basis、changes、各replaces、属性配列、属性オブジェクトの個数の合計は最大32,768。全コンテナで共通の予算とし、宣言数だけで上限を確認してから読む。文字列長はこの個数に含めない。
- 上限はv1の互換性条件。受信側・送信側とも適用し、切り詰めない。

## レコード構造（順序固定）

| 要素 | 表現 |
|---|---|
| magic | `48 55 50 00`（HUP＋NUL） |
| format_version | u32、現在1 |
| cluster_id / content_id / root_id / update_id / author_node_id | それぞれ32 bytes、左記順 |
| edited_at | i64 |
| action | u8: create=0、edit=1、delete=2、keep=3、restore=4 |
| basis | ID集合 |
| changes | u32個数＋以下のchange列 |

changeはfield・value・replaces（ID集合）の順。

field: u8タグ（body=0、attribute=1、kind=2）。attributeはnamespace文字列とkey文字列、kindはname文字列が続く。field順はbody、attributeの(namespace,key)辞書順、kindのname辞書順。重複を拒否。

value: u8タグ。body文字列=0、属性値=1、不存在=2、kind false=3、kind true=4。0には文字列、1には下記属性値が続く。fieldに適合しないvalueは拒否。

属性値: u8タグ。null=0、false=1、true=2、number=3、string=4、array=5、object=6。

- number: 正規化した10進数の文字列。符号は負数のみ`-`、係数は先頭・末尾ゼロなし、指数が非ゼロなら小文字`e`とi64の10進表記（`+`や先頭ゼロなし）を付ける。ゼロは`0`。例: `12345e-2`、`1e2`。浮動小数点を経由しない。
- string: 文字列。
- array: u32個数＋属性値列。
- object: u32個数＋キー文字列・属性値のペア列。

末尾bytes、未知タグ、重複、不正数値、非canonicalな並びや数値を拒否する。codecは構造を検証し、依存関係・action条件・root等の意味はdomainが検証する。

## 同一性・fixture

SHA-256はmagicを含むレコード全bytesに対して計算する。digestは破損検出用で、署名・認証には使わない。同ID受理時は元bytesの一致も確認する。保存・中継で元bytesを維持する。

`fixtures/create-v1.hex` と `fixtures/create-v1.sha256` はPython標準ライブラリによる独立生成。`python3 design_checks/codec_fixture.py` で照合できる。Rustのround-tripだけで互換性を判断しない。

SHA-256実装はRustCryptoの[sha2 0.10.9](https://docs.rs/sha2/0.10.9/sha2/)を使用する。
