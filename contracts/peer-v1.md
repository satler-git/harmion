# Peer HTTPS v1

認証済みのノード間で、元の更新bytesと永続カーソルを交換する初期形式。App用credentialは受け付けない。

## TLSと承認

- native hostはRustlsでHTTPS相互TLSを行う。管理証明には別途標準のEd25519を使い、transport認証と管理証明の検証を分ける。平文Peer HTTPは使わない。
- 管理権限ManageNodesで、同一clusterのNode IDと公開証明書DER・接続先を明示的に承認する。通常のRead/Write allでは承認できない。
- TLSでは承認済み証明書をtrust anchorとし、有効期間・署名・用途を検証する。さらに接続で得たleaf証明書が登録DERそのものと一致することを確認する。HTTPヘッダーから証明書情報を受け取らない。
- SNI/証明書名は`harmion.local`、接続先は登録したIP:port。DNS名・URLをNode IDや認証情報として扱わない。外部CA・プロキシ・HTTPリダイレクトは使わない。
- 各Peer要求はclusterとNode IDを持つが、それだけでは認証しない。TLS証明書と永続登録をトランザクション内で照合する。
- 有効な退役は同じNode IDの通常再承認を拒否する。既存のTLS接続でも、データ要求ごとに現在の退役・保留状態を適用する。署名済み管理証拠の交換は別扱い。承認済みIDの証明書差し替えも拒否する。
- サーバーは新規接続ごとに承認済み証明書集合を取得する。承認後の再起動は不要。保存済み過去更新は作成者の退役だけでは消さない。

daemonは署名付き参加・退役・却下証明を第三ノードへ転送する。登録JSON v3、schema v5、管理commandの追加と保留・回復の契約は[membership-v1.md](membership-v1.md)を参照。直接承認のみの低レベルAPIは既存transportテスト用に残すが、管理証明を初期化したDBでは無署名のapprove/retireを拒否する。native鍵の初期導入はUnixの所有者限定ファイルで実装している。Peer接続できることはAppの管理権限を意味しない。

## HTTP

`POST /peer/v1`。Hostは`harmion.local`、Origin付き要求とquery stringは拒否する。Content-Typeは`application/vnd.harmion.peer-v1`。本文は下記のバイナリ形式。responseはno-store。

接続数16、ヘッダー数32、HTTPヘッダーバッファ16 KiB、接続処理期限15秒。本文は最大4 MiB＋8 KiB。長い接続は再接続・カーソル照会で続行する。接続終了時にACKが失われても、commit済みカーソルで再開できる。

## メッセージ

record-v1と同様、整数はbig-endian、IDは32 bytes。seqはu64。末尾bytes、未知version/tag、上限超過は拒否する。ページ内レコードは再符号化せずそのまま運ぶ。

共通ヘッダー: magic（request=`HPQ\0`、response=`HPS\0`）、u32 version=1、cluster ID、送信ノードID。続けてu8タグとpayload。

| request tag | payload |
|---|---|
| 0 Status | なし |
| 1 Cursor | 送信ログのepoch |
| 2 Page | 取得したいログのepoch、after、固定high |
| 3 Accept | ページ |
| 4 MembershipStatus | なし |
| 5 MembershipCursor | 管理ログのepoch |
| 6 MembershipPage | 管理ログのepoch、after、固定high |
| 7 MembershipAccept | 管理証明ページ |

| response tag | payload |
|---|---|
| 0 Status | epoch、high、u32のrecord version・record byte上限・page record byte上限・page件数上限 |
| 1 Cursor | epoch、through |
| 2 Page | ページ |
| 3 Ack | epoch、commit済みthrough |
| 4 CursorMismatch | epoch、現在の永続カーソル |
| 5 Rejected | 失敗したupdate ID、u8理由（依存不足=0、同ID不一致=1、その他不正更新=2） |
| 6 MembershipPage | 管理証明ページ |

ページ: epoch、after、through、high、u32件数、各entryのseq・u32 record byte長・元のrecord bytes。最大128件、record bytes合計4 MiB。境界・順序・現在カーソル・依存を検査し、一部でも失敗すれば全体をrollbackする。Rejectedを返すために失敗トランザクションをcommitしてはならない。構造をdecodeできない入力はHTTP 400、認証・参加資格違反は403、保存失敗は500。

record形式と上限の不一致は停止する。Statusの独立Python fixtureを`contracts/fixtures`に保存している。

## 双方向ラウンド

開始時に両方向のhighを固定する。自ログの相手側カーソルと、相手ログのローカルカーソルを取得し、小さいページ単位でupload/downloadを交互に進める。native hostでは同一Peerへの自発ラウンドを直列化する。別Peerは並行して進められる。相手から開始された転送もあり得るため、古いafterは現在カーソルへ追従する。

別の転送が固定highより先まで進んだ場合は、送信側の現在highと同一epochを確認して区別する。ログが同一epochのまま巻き戻った矛盾はエラーにする。新規更新は次のラウンドへ回す。daemonには起動時・周期・手動同期があり、周期同期は最大4ラウンドを並行実行する。保存直後・OS復帰・更新通知は未実装。

## 検証範囲

transportの区別はschema v4 / 登録JSON v2で追加した。現在の登録JSON v3は署名検証鍵と共通の初期信頼証明も持つ。`binding.transport`は`https`（certificate/endpoint）または`iroh`（endpoint_id/relay_url）。daemonから新しいpeer.jsonを取得する。既存SQLiteのv3証明書登録はTLS鍵として移行する。同じbytesでもTLS証明書とiroh公開鍵を混同しない。既存Node IDへの別鍵差替えは拒否する。

irohはALPN `harmion/peer/1`で、1接続の最初の双方向streamにHPQ要求、HPS応答を1つずつ送る。各方向のFINがframe境界。既存codecとサイズ上限を共用し、接続のremote Endpoint IDと承認済み鍵を照合する。不正encoding/未承認の場合は接続を閉じる。呼出し側は期限付きで失敗し、保存が成功したか不明なら同じページを再送する。

TLS型を含まない`PeerTransport`を介して同じ固定highラウンドを実行する。pull応答保存時もtransaction内で有効なPeer登録を再検査する。irohのIP/portは参加資格の根拠にせず、Endpoint IDへの探索結果として扱う。

実TCP/TLSとSQLiteを使い、並行編集、削除競合、同時開始、ACK再送、DB再open、未承認鍵、偽Node ID・cluster、偽サーバー証明書、接続中の退役、拒否ページの原子性をテストする。`design_checks/two_process_sync.py`はdaemon/CLIによる独立2プロセスを、標準のHTTPSと`--iroh`のローカルリレー経路で検証済み。5プロセスの検証範囲と管理証明の不足は[実装全体レビュー](../docs/records/implementation-review.md)を参照。

実装参照: [Rustls WebPkiClientVerifier](https://docs.rs/rustls/0.23.46/rustls/server/struct.WebPkiClientVerifier.html)、[rcgen CertificateParams](https://docs.rs/rcgen/0.14.10/rcgen/struct.CertificateParams.html)。
