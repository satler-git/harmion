# 所有者認証・承認の改善案

状態: **Phase 1（R2端末承認・監査）とPhase 2（ノード検証SSH assertion）は実装済み（R56）。** WebAuthn/Passkey、OS鍵保管、Cluster Owner、thresholdは未実装。この文書は実装の根拠と残りの設計を記録する。

## 1. 要約

- メモのB（Owner Key + challenge署名）は方向として妥当。C（Cluster Ownerポリシー）とD（常駐Owner Agentを前提にする構成）は、現時点では採用しない。
- 先に直すべきは認証要素の追加ではなく、**認証結果に与えている権限の広さ**。現在は管理セッションが全管理操作を実行でき、UIの確認画面はサーバー側で強制されていない。
- **Owner Approval v1** は管理操作をR0/R1/R2に分類し、R2（権限・信頼・データ持ち出し）だけ起動端末での単回承認を必須にする。セッション窃取・XSS・CSRFだけではR2を完結できない。
- **Phase 2** で、ノード自身が所有者assertionを検証する。OpenSSHのSSHSIG署名を使い、`ssh-agent`・macOS Keychain・FIDO2 `sk-ssh-ed25519`・PIVカードをそのまま使える。ノードは`ssh-keygen -Y verify`で公開鍵と署名だけを検証し、`admin.secret`やセッションを盗んだだけではR2を実行できない。同一OSユーザーの任意コード実行は、agentを操作できる場合には依然として止められないため、FIDO2/PIVのper-use presenceを推奨する。
- WebAuthn/PasskeyはブラウザUXの選択肢として残すが、現在の`127.0.0.1` originとCAでは動かないため、SSHの後のスパイクとする。GPG/OpenPGPは既存ユーザー向けの任意backend、Nostrは公開鍵+署名statement+remote signerという**パターンだけ**を借り、secp256k1や公開relayは採用しない。
- クラスタ全体の所有者鍵は作らない。クラスタの変更は既存のmembership署名でノードを認証し、仕様6.3の「承認元ノードに委ねる」モデルを維持する。

## 2. 現状（コードで確認した範囲）

| 項目 | 現状 | 問題 |
|---|---|---|
| 管理資格情報 | ノード初期化時に32バイトの`admin.secret`を発行。`bootstrap_admin`の期限は`i64::MAX` | 無期限・ローテーション手段なし。コピーされると恒久的な所有者権限になる |
| 管理UIセッション | 起動リンクは2分・単回。セッションはメモリ内で30分固定 | アイドル期限なし。1セッションが全管理操作を実行できる |
| 管理APIの認可 | Bearerを定数時間比較し、パスごとに`api::dispatch`が操作を実行 | 操作ごとの所有者意思確認がない。セッションを持つ者はUIを経由せず直接呼べる |
| 既存の確認UI | Peer承認などは`/peer-preview`→`/peer-confirm`の単回ticket | ticketは「プレビュー済み」の記録で、所有者が確認した証拠ではない。セッション保持者はpreviewとconfirmを直接呼べる |
| ノード側の主体 | 管理ツールは`Grants::administrator()`を持つ通常のAppPrincipal | 「所有者」はアプリ資格情報と同義。管理権限の拡大・ノード承認・exportを区別しない |
| ノードの秘密 | `admin.secret`、`iroh.secret`（iroh兼membership署名鍵）、`private-key.der`、`oauth-identity.json`が同じ所有者限定ディレクトリに平文で並ぶ | 管理資格情報だけをOS鍵保管へ移しても、ノード署名鍵とTLS鍵は残る。バックアップにも全部入る |
| クラスタ | Ed25519のAdmission証明、trust root、退役、replay対策を実装 | 所有者承認は承認元ノードに委ねる（仕様6.3）。クラスタ全体の所有者主体は存在しない |
| Worker | Cloudflare Access + デプロイ発行Bearer、読み取り専用 | nativeのOS所有者モデルを適用しない別系統。今回の変更対象外 |

## 3. メモの案をそのまま採用しない理由

### 3.1 脅威モデルが一つに混ざっている

メモの目標1〜5は、少なくとも次の別々の脅威を一つの機構で解こうとしている。必要な対策は一致しない。

| 脅威 | 現状の防御 | メモの案 | この提案 |
|---|---|---|---|
| 別OSユーザー | 0600の所有者限定ファイル、証明書固定のloopback HTTPS | Owner Key | 維持 |
| ネットワーク越しの攻撃者 | pinned TLS、Bearer、Host/Origin検査 | Owner Key | 維持 |
| ブラウザセッションの窃取・XSS・CSRF | 30分の全権セッション。UIの確認は強制でない | WebAuthn assertion | R2を端末承認で強制 |
| 同一OSユーザーの任意コード | 防げない（仕様6.3で保証外） | WebAuthnで防げるように読める | 防げないと明記。Phase 2は資格情報窃取だけを止める |
| 端末盗難・バックアップ漏洩 | 平文のノードディレクトリ | OS鍵保管 | まずバックアップ暗号化。鍵保管はPhase 2以降 |
| 複数端末・複数管理者 | ノード単位の承認 | Cluster Owner | 既存membershipのノード署名 |

特に、WebAuthnは「ブラウザのページが正当な管理UIである」ことを前提にする。管理UIがXSSや拡張機能で乗っ取られていれば、任意のchallengeに署名させられる。パスキー自体がユーザーに表示するのは「localhostにサインイン」であり、操作内容の同意ではない。メモのBはこの点を書いていない。

### 3.2 仕様が決めた委任モデルを覆している

仕様6.3は「ノード追加・アプリ認可は所有者が管理ツールで明示的に承認する。他ノードは信頼済みノードの参加証明を検証し、ユーザーによる承認の確認は承認元ノードに委ねる」と定める。メモの目標1「所有者の承認を、特定のノードのファイルではなく、クラスタのポリシーと所有者鍵で表す」はこれを反転させるが、どの攻撃が防げるようになるのか、なぜ委任では足りないのかが書かれていない。

さらに、仕様ではアプリ認可（grant）はノードローカルで同期しない。grant変更やProvider承認にクラスタ全体の所有者署名を要求すると、次のどちらかになる。

- 署名はするが各ノードは自分のgrantを変えない（クラスタ所有者の意味がない）。
- grantをクラスタで同期する（ローカル認可という現在の設計を大きく変える）。

メモはこの区別をしていない。

### 3.3 既存のmembershipと二重の権威になる

membership v1はEd25519署名、root-firstのAdmission証明列、退役、未確認証明、replay対策、identity pinを持つ。Owner Key + challengeは同じ問題を別のPKIで解くことになる。二つの権威があると、次を決める必要がある。

- 分断中にOwner Policyとmembershipが食い違ったとき、どちらを優先するか。
- ポリシーを同期していないノードをfail closedにすると、オフラインのノードが管理不能になる。メモの目標3「オフラインのノードが後から承認結果を検証できる」と、Cの「同期しないノードはfail closed」は両立しない。
- 所有者の追加・削除・threshold変更の順序を、ノード間でどう決めるか。

メモは「決めていないこと」に一部を挙げるが、実装前に解くべき競合規則としては扱っていない。

### 3.4 WebAuthnが現在の管理UIの構成では動かない

- 現在の管理UIは`https://127.0.0.1:<ephemeral>`で、証明書のSANはIPアドレス。WebAuthnのRP IDはIPアドレスを受け付けない。
- CAのNameConstraintsは`127.0.0.1`だけを許可し、DNS名を除外している。`https://localhost`へ変えるには、leafのDNS SAN、CAのNameConstraints、Hostヘッダー検査、既存CAの再発行・再信頼が必要。
- ポートは毎回変わる。RP IDは`localhost`で固定できるが、originはポートを含むため、検証側は`https://localhost:*`を許可する設計になる。これは別のローカルサーバーが本物の管理UIを装える余地を作るため、セッションとの結び付けが必須になる。
- これらはメモの「セキュリティ上の検討点」に一行も書かれていない。Phase 1の前提が未検証。

### 3.5 OS鍵保管とOwner Agentのコスト

- LinuxのSecret Serviceは秘密の保管庫であり、署名APIではない。TPMは常にあるわけではない。macOSのKeychain、WindowsのCredential Manager、Android Keystore、iOS Secure Enclaveで実装が分かれる。
- Owner Agentは新しい常駐プロセスで、ブラウザからAgentへの認証がまたloopbackの認証問題になる。Agentを乗っ取れば署名を代理できる。
- 回復鍵をファイルに置けば、メモが解決しようとした「平文ファイル」の問題がそのまま戻る。thresholdは鍵紛失時のロックアウトを増やす。
- ノードの`iroh.secret`はmembership署名鍵も兼ねる。管理資格情報だけを鍵保管へ移しても、ノードなりすましの鍵は平文で残る。秘密の全体像を扱う必要がある。

### 3.6 移行が未解決

- 既存`admin.secret`ノードで、最初のOwner Keyを誰が承認するか（メモ自身が未決）。
- `admin.secret`をbootstrapと回復に残すなら、恒久的なバイパスが残る。消すなら、鍵紛失時の回復経路を先に作る必要がある。
- メモにはwire形式、保存schema、受け入れテスト、ロールバックがない。

## 4. 提案: Owner Approval v1

### 4.1 原則

1. 信頼の根拠はOSユーザーのまま（仕様6.3を維持）。新しいログイン基盤を作らない。
2. 認証（誰か）と承認（何をしてよいか）を分ける。セッションは「読み取りと操作の提案」まで。
3. 権限・信頼・持ち出しに関わる操作は、操作内容に結び付いた単回の所有者確認を必須にする。
4. 確認はブラウザの外で行う。ブラウザセッションの窃取やXSSだけではR2を完結できない。
5. クラスタは既存membership署名でノードを認証する。Cluster Ownerは作らない。
6. 同一OSユーザーの任意コード実行は防げないと明記する。ノード検証が止めるのは、`admin.secret`やセッションの窃取だけでのR2であり、agentを操作できるマルウェアは止めない。

前提:

- OSアカウント境界は有効。別ユーザーや遠隔の攻撃者は、資格情報なしに所有者限定ファイルやloopbackへ到達できない。
- 同一OSユーザーの任意コード実行は保証外（仕様6.3）。ブラウザは侵害され得るが、ノードと管理サーバーのコードは侵害されていないものとする。
- Ed25519、SSH署名、TLSの暗号が破られない。

### 4.2 操作の分類

| クラス | 対象 | 必要なもの |
|---|---|---|
| R0 読み取り | overview、members、contents、inspect、metrics、connect-list、interpreter-list/describe、peer-preview | セッション |
| R1 内容変更 | resolve、merge、merge-base、oauth-register、node sync | セッション + 監査 |
| R2 権限・信頼・持ち出し | change-grants、node approve/join/retire/confirm-retirement/dismiss-retirement、peer-confirm、oauth-consent、connect-approve、interpreter-approve、export | セッション + 操作単位の端末承認 + 監査 |

削除・復元（`resolve`のDelete/Restore）をR1に置くかR2に上げるかは未決とする。履歴は残るが、失いたくない場合はR2にする。所有者credential自体の管理（`owner-enroll`、`owner-revoke`、`owner-recover`、`owner-enforce`）もR2とする（§4.8.10）。

### 4.3 R2の承認フロー

```text
Browser -> Admin server: POST /change-grants {app_id, grants}
Admin server: 要求をcanonical化し、要約とrequest hashを作る
Admin server -> Node: owner-challenge（operation/target/args hash/session）
Admin server -> 起動端末: 操作・対象・差分を表示し、[y/N]を待つ（最大120秒）
Owner -> 起動端末: y
Admin server -> SSH agent: statementへ署名（Phase 2。touch/PIN/確認）
Admin server -> Node: change-grants + assertion
Node: challenge・署名・本文hashを検証し、同じtransactionで実行
Admin server -> Browser: 成功
```

- 承認はプロセス内の単回ticketとし、`request hash`・nonce・期限（120秒）に結び付ける。使用時に消す。
- 承認は管理サーバーの単一チャネルで直列化する。複数のR2が同時に来た場合は、前の端末確認が終わってから次を表示する。
- 要約はサーバー側で型付きデータから生成し、制御文字を除去し、長さを制限する。ブラウザが送った文字列をそのまま端末へ出さない。
- 起動端末がTTYでない場合、R2は既定で拒否する。自動化・E2E用に`--approval=stdio`を明示した場合だけ、監督下のstdinから承認を読む。`--approval=deny`は常に拒否。
- UIの既存確認画面はUXとして残すが、強制点はサーバー側の端末承認とする。
- `/peer-preview`→`/peer-confirm`は維持する。ただし`peer-confirm`も端末承認を通し、ticketは承認後に消費する。期限切れなら410で再プレビューさせる。
- 端末承認（v1）とノード検証（v2、§4.8）は置き換えではなく層。端末承認は「何に同意するか」の表示、SSH assertionは「誰が同意したか」の証明。`enforce=1`では両方を要求する。端末承認だけでは`admin.secret`保持者の直接呼び出しを止められず、署名だけでは署名者に操作内容が見えない。

### 4.4 セッション

- アイドル10分、絶対30分。`/session/end`で明示終了。同時に有効なセッションは1つ。
- R2はセッションだけでは実行できない。セッションの新しさを理由に端末承認を省略しない。
- Phase 2bで、ブラウザが生成した鍵で各要求に署名するDPoP類似の束縛を検討する。ただしXSSでページ内の鍵を使われると防げないため、R2承認の代替にはしない。

### 4.5 管理資格情報

- `admin.secret`は当面残す。CLIと管理サーバーがノードへ接続するためのルート資格情報であり、ブラウザへ渡さない。0600、ピアへコピーしない、を維持する。
- `harmion --node DIR owner-status`でcredential、enforce、回復コードの有無、監査を表示する（実装済み）。所有者鍵の追加・失効・回復は`owner-enroll`・`owner-revoke`・`owner-recover`で行う（実装済み）。
- `admin.secret`の二段階ローテーションは未実装。実装する場合は、クライアントが新しい32バイトを生成し、そのverifierだけをノードへ送り、新しい`admin.secret`を書き込んだ後にcommitする。commit前に失敗した場合は旧資格情報が有効なままにする。
- 短い有効期限を付けるだけの変更はしない。更新経路なしの期限切れはロックアウトになる。
- OS鍵保管への移動、Unix socket + peer credentialによるローカルIPCはPhase 3以降の候補とする。管理APIを通常のApp APIに統一する現在の仕様を変えるため、単独では行わない。

### 4.6 監査

- 監査はノードDBの`owner_audit`テーブルへ追記する。R2認可時はchallenge消費と同じtransactionで`authorized`を記録し、enroll/revoke/recover/enforceはその操作のtransactionで記録する。操作本体の成否は`result=authorized`とは別である。
- 記録: seq、時刻、actor、operation、target、args hash、credential ID、challenge ID、session ID、result、直前行のhash、entry hash。
- 本文・トークン・秘密・OAuth codeは記録しない。
- CLI `owner-status`で直近の記録を確認できる。直近32件のチェーン整合性を`audit_valid`で返す。管理UIの専用画面は未実装。
- 同一ユーザーはDBを書き換えられるため、これは改ざん検知であって改ざん防止ではない。必要なら外部へエクスポートする。
- ファイル出力はExportや外部監査向けに後から追加する。DBを正とする。

### 4.7 クラスタ

- 参加・退役・却下は、承認したノードが既存のmembership署名で証明する。他のノードはActiveなmemberの署名と証明列を検証する。現在の動作を変えない。
- Cluster Ownerポリシー、クラスタ共通の所有者鍵、thresholdは作らない。grant、Provider承認、接続承認はノードローカルである。
- 将来、遠隔管理や複数管理者が必要になった場合だけ、membershipのstatementに任意のowner attestation（ノード署名 + 所有者鍵署名）を追加する。所有者鍵はノード単位で登録し、クラスタ共通の単一ポリシーにしない。分断時の扱いは既存のmembership規則を再利用する。
- CloudflareはCloudflare Access + デプロイ発行Bearerの別系統を維持する。OS鍵保管を前提にしない。

### 4.8 ノード検証の所有者assertion（Phase 2の本命）

目的は「`admin.secret`や管理セッションを盗んだだけではR2を実行できない」こと。管理サーバーやブラウザではなく、**ノードが署名を検証する**。管理サーバーだけが検証しても、`admin.secret`を持つプロセスは迂回できるため不十分である。

#### 4.8.1 共通の流れ

1. 管理サーバーまたはCLIが、実行したいR2の`operation`・`target`・本文の`args_sha256`・`session`をノードへ送り、challengeを発行してもらう。
2. ノードはchallengeをDBへ保存し、canonical statementを返す。
3. 所有者はstatementをSSH鍵などで署名する。署名の前に、管理サーバー/CLIが操作内容を端末へ表示する。
4. R2要求にchallenge IDと署名を付け、ノードが同じtransactionで検証・消費してから操作を実行する。

```text
POST /app/v1/owner-challenge
{"operation":"change-grants","target":"<app_id>","args_sha256":"<hex>","session":"<hex>"}
-> {"challenge":"<hex>","statement":"<hex>","expires_at":1234567890}
```

```text
POST /app/v1/change-grants
X-Harmion-Owner-Challenge: <hex>
X-Harmion-Owner-Signature: base64url(<SSHSIG PEM>)
<通常の本文>
```

ノードが検査するもの:

- `operation`と`args_sha256`が実際の要求と一致する。`args_sha256`はノードが受け取った本文バイト列から再計算する。`target`はbodyに含まれる対象を識別する監査用の値で、本文の差し替えはargs hashで拒否する。`session`は監査と相関用であり、認可の根拠ではない。
- 署名が登録済みcredentialの公開鍵で検証できる。SSHSIGに埋め込まれた公開鍵のfingerprintも登録済みcredentialと一致する。credentialが失効していない。
- SSHSIGのnamespaceが`harmion-owner`である。
- statementのcluster IDとnode IDが自分と一致する。

canonical statementは、JSONのキー順に依存しない固定形式にする。

```text
HOA1\0
cluster=<hex32>\n
node=<hex32>\n
operation=<ascii>\n
target=<hex32|->\n
args_sha256=<hex32>\n
session=<hex32|->\n
challenge=<hex32>\n
issued_at=<i64>\n
expires_at=<i64>\n
```

`target`は操作対象（grant変更ならapp ID、退役ならnode ID）。対象がない操作は`-`。`session`はWebなら管理UIセッション、CLIならadmin app ID。署名後に本文や対象を差し替えると、`args_sha256`と`target`の検査で落ちる。`session`は監査と相関のための値であり、認可の根拠ではない。認可は署名と、challengeを発行したadmin appの同一性で決まる。

#### 4.8.2 第一候補: SSH署名（SSHSIG）

OpenSSH 8.0以降の`ssh-keygen -Y sign`は、秘密鍵ファイルでも、ssh-agentにある公開鍵でも署名できる。OpenSSHのマニュアルは`-f`に公開鍵を指定し、秘密鍵をagentに置く使い方を明記している。これにより次をそのまま使える。

- `ssh-agent`、macOS Keychain、1PasswordなどのSSH agent
- FIDO2の`sk-ssh-ed25519@openssh.com`（署名ごとにtouch）
- PIVスマートカード（PIN）
- gpg-agentのSSH support（OpenPGPカードを含む）

署名はSSHSIG形式で、namespaceによるdomain separationがある。ノード側は`ssh-keygen -Y verify`で検証する。Rustの`ssh-key` crateで検証する案もスパイクしたが、依存を増やさず、署名側と検証側を同じOpenSSH実装に揃える方を採用した。`ssh-keygen`が無いホストではR2をfail closedする。ノードは秘密鍵もagentも持たない。

```sh
# 署名側（管理サーバー/CLIが呼ぶ）
ssh-keygen -Y sign -f ~/.ssh/id_ed25519.pub -n harmion-owner /path/to/statement

# ノード側（登録済み鍵をallowed_signersに入れ、statementをstdinで渡す）
ssh-keygen -Y verify -f allowed_signers -I owner -n harmion-owner -s statement.sig \
  -O print-pubkey < statement
```

設計時点の確認: 手元のOpenSSHで`ssh-keygen -Y sign`→`-Y verify`の往復、namespace不一致の拒否、Ed25519署名が約306バイトであることを確認した。Rustの`ssh-key` 0.6.7でもOpenSSH生成のSSHSIGを検証できることを確認したが、採用は上記の外部検証とした。SSHSIGはHTTPヘッダーに収まる。

採用理由:

- 既存の鍵管理・agent・ハードウェア鍵の資産をそのまま使える。新しい秘密鍵の保管場所を作らない。
- Ed25519は既にrepoで使っている。secp256k1などの曲線を追加しない。
- 署名側はOpenSSHに任せ、ノード側は検証だけを実装する。ノードに署名機能やagent接続を追加しない。
- FIDO2/PIVを使えば、パスフレーズのキャッシュに依存せず、署名ごとのuser presenceを得られる。

注意:

- パスフレーズ付きのソフトウェア鍵をagentやKeychainに入れただけの場合、unlock中はagentが無確認で署名できる。これはper-useのuser presenceではない。`ssh-add -c`の確認、FIDO2のtouch、PIVのPINのいずれかを推奨する。
- `ssh-key` 0.6.7は`ed25519-dalek 2`に依存し、repoの`ed25519-dalek 3`と別バージョンが併存する。ビルドサイズは増えるが型の互換性問題はない。0.7系はpre-releaseなので、最初は安定版を使う。
- `ssh-keygen`が無い環境ではR2をfail closedにする。WindowsはOpenSSHクライアントを任意依存とする。

#### 4.8.3 仕組みの比較

| 方式 | 鍵・署名 | user presence | ノード検証 | 移植性 | 追加依存 | 評価 |
|---|---|---|---|---|---|---|
| SSH SSHSIG | Ed25519/ECDSA/RSA、agent、FIDO2 `sk-*`、PIV | `ssh-add -c`、touch、PIN | `ssh-keygen -Y verify` | Unix/Windows/macOS | 小 | **第一候補** |
| GPG/OpenPGP | RSA/Ed25519/Cv25519、gpg-agent、OpenPGPカード | pinentry、PIN | `sequoia-openpgp`または`gpg` | Unix/Windows | 大 | 既存GPGユーザー向けの任意backend |
| WebAuthn/Passkey | P-256/Ed25519、platform/security key | 生体/PIN、touch | `webauthn-rs` | ブラウザ | 中 | ブラウザUXは最良。origin/CAの変更が前提 |
| FIDO2直接（CTAP2） | P-256/Ed25519 | touch/PIN | `ctap-hid`等 | USB/udev | 中 | SSHの`sk-*`で代替できるため不採用 |
| macOS Keychain / Secure Enclave | P-256 | Touch ID/パスワード | native helper | macOSのみ | 大 | Phase 3 |
| Windows Hello / CNG | P-256/RSA | 生体/PIN | native helper | Windowsのみ | 大 | Phase 3 |
| Linux TPM2 | P-256/RSA | PIN | `tss-esapi`等 | TPM必須 | 大 | Phase 3 |
| Nostr（secp256k1 Schnorr） | secp256k1 | 生鍵はsoftware、NIP-46は端末 | `secp256k1` | relay依存 | 中 | パターンだけ採用 |
| 生のEd25519鍵ファイル | Ed25519 | なし | `ed25519-dalek` | 全OS | なし | 緊急回復のみ |

#### 4.8.4 GPGを第一候補にしない理由

- 検証スタックが重い。`sequoia-openpgp`は大きな依存で、暗号backendの選択も増える。外部`gpg`に頼るなら実行時依存とkeyringの分離が必要になる。
- 署名の正規化、副鍵、失効、keyringの扱いがSSHより複雑。管理認証のために新しい鍵管理を覚える必要がある。
- OpenPGPカードは、gpg-agentのSSH support経由でSSH鍵として使えることが多い。まずSSHで受けられれば、GPG専用backendを最初から作る必要はない。
- GPGを既に使っている所有者のために、`mechanism=gpg`を後から追加するのは可能。ただし最初の実装を二重にしない。

#### 4.8.5 WebAuthn/Passkeyの位置づけ

- ブラウザのUXは最良。ただし§3.4のとおり、現在の`https://127.0.0.1:<port>`とCAのNameConstraintsでは動かない。`https://localhost:<port>`、DNS SAN、CAの再発行・再信頼が必要。
- `webauthn-rs` 0.6.2でノード側の検証は可能。challengeはstatementのhash、RP IDは`localhost`、originは`https://localhost:*`をセッションに結び付けて検証する。
- パスキー同期は端末束縛を弱める。既定は端末内credentialとし、同期パスキーは明示選択にする。
- SSH署名の後に、ブラウザ用の追加mechanismとしてスパイクする。必須にしない。CLIとヘッドレス環境のフォールバックは残す。

#### 4.8.6 Nostrから借りるもの、借りないもの

借りる:

- 公開鍵を所有者identityにする。
- 操作をcanonical statementにして署名する。
- `created_at`と`expiration`に相当する鮮度情報をstatementへ入れる。
- statementのhashをIDとして監査に残す。
- 署名者をクライアントから分離する（remote signer）。

借りない:

- secp256k1 Schnorr。Ed25519で足りる。
- 公開relay。管理認証を第三者relayの可用性・メタデータに依存させない。
- NIP-46のリレー依存と生鍵のsoftware管理。
- 署名イベントの汎用配布。ノードは自分のchallengeだけを検証すればよい。

将来、電話や別端末での承認が欲しい場合は、NIP-46風のremote signerを**認証済みの直接チャネル**で実装する。署名者は要求の要約を表示し、ノードが要約に署名した要求を送る（WYSIWYS）。

#### 4.8.7 enrollment、失効、回復

- 最初のenrollmentは**offlineのhost専用コマンド**で行う。daemonを停止し、`harmiond owner-enroll --ssh-key ~/.ssh/id_ed25519.pub`がDBを直接開いて公開鍵とfingerprintを保存し、`enforce=1`にする。最初のenrollmentを通常のApp APIに置かない。これにより、`admin.secret`だけを盗んだ相手はノードAPIから所有者鍵を登録できない。
- 2つ目以降のenrollmentはonlineの`owner-enroll`で行い、既存credentialのassertionを必須にする。盗まれた`admin.secret`だけでは攻撃者の鍵を追加できない。
- 回復コードを最初のenrollmentで一度だけ表示し、hashだけをノードに保存する。`owner recover`はコード + TTYで全credentialを失効し、新しい鍵を登録する。コードはオフライン保管とする。
- `owner revoke`は既存assertionを必要とする。最後のcredentialの失効は、回復コードがある場合だけ許可する。ロックアウトを避ける。
- 複数credential（ノートPC、FIDO2、バックアップ）を許す。既定は1-of-N。thresholdはPhase 3。
- `enforce=1`後の`admin.secret`はR0/R1とchallenge発行だけに使える。R2はassertion必須。`admin.secret`のOS鍵保管への移動は別の改善として扱う。
- offline enrollmentはOSユーザーのローカルアクセスを信頼する。同一ユーザーのマルウェアは、この段階でも防がない。

#### 4.8.8 何を防ぎ、何を防がないか

防ぐ:

- `admin.secret`やセッションの窃取だけでのR2。
- 署名のreplay、別ノード・別クラスタへの流用、操作・本文の差し替え、失効鍵の使用、期限切れchallengeの使用。

防がない:

- unlock中のagentを使える同一ユーザーのマルウェア。`ssh-add -c`、FIDO2 touch、PIV PINで緩和するが、マルウェアがagentを直接使えるなら同じ。
- 管理サーバーや端末表示を書き換えられるマルウェア。
- 利用者が悪意ある要約を承認するソーシャルエンジニアリング。
- ノード自体の侵害。
- パスフレーズをKeychainに入れただけでper-use確認がないソフトウェア鍵は、実質「持っているだけ」に近い。

同一ユーザーのマルウェアまで考えるなら、別端末のremote signerで要約を表示し、その端末で承認するのが唯一筋の良い方向。ただしノードが署名した要約を送るWYSIWYS、認証済みの直接チャネル、端末紛失時の失効が必要で、Phase 3の課題とする。

#### 4.8.9 trusted display

署名は鍵の所持を示すが、何に同意したかは示さない。署名者のプロンプト（touch、PIN、agent確認）は操作内容を表示しない。そのため、R2では先に管理サーバー/CLIが端末へ要約を表示し、所有者が確認してから署名する。要約はサーバー側で型付きデータから生成し、ブラウザが送った文字列をそのまま出さない。remote signerを使う場合は、署名者側が要約を表示できるよう、ノードが要約に署名した要求を送る。

#### 4.8.10 データとAPI

| テーブル | 内容 |
|---|---|
| `owner_credentials` | id、mechanism（1=ssh）、public_key、fingerprint、label、created_at、last_used_at、revoked |
| `owner_challenges` | id、statement_hash、session、operation、target、args_hash、issued_at、expires_at、used_at |
| `owner_policy` | singleton、enforce、recovery_hash、updated_at |
| `owner_audit` | seq、at、actor、operation、target、args_hash、credential、challenge、session、result、prev_hash、entry_hash |

| API / コマンド | クラス | 内容 |
|---|---|---|
| `harmiond owner-enroll`（offline） | host専用 | 最初のcredentialを登録し、`enforce=1`にする。daemon停止中に実行 |
| `owner-status` | R0 | credential一覧、enforce、回復コードの有無、直近の監査 |
| `owner-challenge` | R0 | R2用のchallengeとstatementを発行。レート制限する |
| `owner-enroll` | R2 | 2つ目以降のcredential追加。既存assertion必須 |
| `owner-revoke` | R2 | 既存assertion必須。最後のcredentialは回復コード必須 |
| `owner-recover` | R2 | 回復コード + TTY |
| `owner-enforce` | R2 | enforceの切り替え。既存assertion必須。全credentialを失った場合は`owner-recover`を使う |

- schema migrationでは空テーブルを作り、既存ノードは`enforce=0`のまま動かす。所有者がofflineで最初のcredentialを登録した時点で`enforce=1`にする。
- R2の検証とchallenge消費は、操作を実行する前のtransactionで行う。操作が失敗してもchallengeは消費済みで、再実行には新しい承認が必要になる。
- `export`のように複数transactionにまたがる操作は、最初の認可transactionで消費する。途中失敗でchallengeが消費されたままになるのは許容する。
- SSHSIGはHTTPヘッダーに載せる（base64url）。WebAuthnを追加するときはassertionが大きくなるため、JSON本文のenvelopeを併用する。

### 4.9 Phase 2b以降

- Phase 2b候補: WebAuthn/PasskeyのブラウザUX。§3.4のorigin/CAの変更とスパイクが前提。
- Phase 3: GPG/OpenPGP backend、OS鍵保管・TPM・Secure Enclave、複数所有者、threshold、鍵ローテーション、遠隔管理。
- ノードの秘密（`iroh.secret`、`private-key.der`）のOS鍵保管・暗号化バックアップ。
- これらは実需要が確定してから設計する。Cluster Ownerを先に作らない。

## 5. 実装順

1. **R2の分類と端末承認**（管理サーバー、TTY/stdio/deny、fail closed、同時1件） — 実装済み（R56）。
2. **ノード検証のSSH assertion**（`owner_credentials`/`owner_challenges`/`owner_policy`/`owner_audit`、`owner-challenge`、R2ヘッダー検証、offlineの`owner-enroll`、onlineの追加enroll/revoke/recover、`ssh-keygen -Y verify`、単体テスト） — 実装済み（R56）。
3. **監査の表示と検証**（hash chain、`owner-status`の監査表示）。UIの専用画面と外部エクスポートは未実装。
4. **セッション期限**（アイドル10分、絶対30分）は実装済み。`admin.secret`の二段階ローテーションは未実装。
5. **WebAuthn/Passkeyのスパイク**（localhost origin、CA、ブラウザ互換性、ノード検証） — 未着手。
6. **GPG/OS鍵保管/TPM**は、SSHで不足する具体的な要求が出たときだけ。
7. クラスタのowner attestationは、遠隔管理・複数管理者の具体的な要求が出たときだけ。

## 6. 受け入れテスト

実装済みのテスト:

- `crates/store-sqlite/tests/owner.rs`: 正しいSSH署名で成功、同一challengeの再利用拒否、別bodyへの流用拒否、statusのenforce/監査、recovery codeの置換と旧code拒否。
- `crates/net-native/tests/owner_https.rs`: `enforce=1`でassertionなしのR2が403、正しい鍵のonline enroll成功、未登録鍵の署名が403。
- 実プロセス: `harmiond init`→`owner-enroll`→`run`→`owner-status`、PTY上のCLI `owner-enroll`（端末承認→SSHSIG→ノード検証→credential追加）。

残りの確認項目:

- R2をセッションだけで直接呼ぶと403で、DBが変化しない。
- 端末で拒否すると403で、DBが変化しない。
- 承認待ちの間に別のR2を送ると429。
- TTYなしの既定でR2が拒否される。`--approval=stdio`では監督下の入力だけが承認になる。
- 承認要約に制御文字・過大な文字列が混入しない。
- 別のnamespace、別のoperation/target、別のcluster/node、期限切れchallengeを拒否する。
- FIDO2 `sk-ssh-ed25519@openssh.com`の公開鍵を実機で検証できる。
- 既存の`design_checks/admin_process.py`が`--approval=stdio`で通る（Nix dev shellで成功済み）。

## 7. 未決事項

- 削除・復元をR2に上げるか。
- 監査の外部エクスポート形式（JSON Lines、Exportへの同梱など）。
- `--approval=stdio`の許容範囲（テスト専用にするか、監督下の自動化に使うか）。
- 既定の署名手段をソフトウェアSSH鍵にするか、FIDO2/PIVを必須にするか。
- 回復コードと2つ目のバックアップ鍵のどちらを既定にするか。
- offlineの`owner-enroll`のUX（`harmiond owner-enroll`か`harmion owner enroll --offline`か）。
- 全credentialと回復コードを失った場合のbreak-glass（バックアップ復元か、offline resetか）。
- `admin.secret`のOS鍵保管への移動をどこまでやるか。
- WebAuthn/PasskeyをPhase 2bに入れるか、SSHだけで足りるか。
- ノード鍵とバックアップの暗号化を、所有者認証と同じ時期に扱うか。
- Workerの遠隔管理を将来追加する場合の認証（今回の対象外）。

## 8. メモとの関係

| メモの項目 | この提案 |
|---|---|
| A. Node OwnerとCluster Ownerを分ける | Node Ownerは維持。Cluster Ownerは作らない |
| B. Owner Key + challenge署名 | 方向は維持。Phase 2でSSH SSHSIGのノード検証として具体化 |
| C. Owner Policy | 採用しない。grantはノードローカル、クラスタ変更はmembership署名 |
| D. Owner Agent / 端末統合 | 必須にしない。SSH agent/FIDO2/PIV/WebAuthnを署名backendとして選ぶ |
| Phase 1（OS鍵保管→WebAuthn） | 順序を反転。先にセッション権限の縮小と端末承認、次にノード検証 |
| Phase 2（Cluster Owner） | 採用しない。必要ならmembership上の任意attestationとして後付け |
| Phase 3（threshold） | 実需要が出るまで保留 |

## 参照

- [所有者認証の将来設計メモ](owner-authentication-memo.md)
- [仕様 §6.3 所有者とローカル管理の信頼境界](specification.md#63-所有者とローカル管理の信頼境界)
- [実装全体レビュー](implementation-review.md)
- [引継ぎ](implementation-handoff.md)
