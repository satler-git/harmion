# 所有者認証の将来設計メモ

状態: **メモのみ。未実装。** 現在のv1は`admin.secret`と管理UIの起動リンクを使う。このメモは、クラスタ全体の所有者認証とファイル以外の秘密保管へ移行するときの検討材料であり、実装上の約束ではない。

## 現状の限界

- 所有者はノードごとの`admin.secret`（Unix所有者限定ファイル）でbootstrapする。管理UIは`harmion-admin`が発行する2分・単回の起動リンクを開き、30分のメモリ内セッションを使う。
- 管理APIはBearerヘッダーで、Cookieを使わない。ブラウザは`admin.secret`を受け取らない。
- 同じOSユーザーで動くプログラムは所有者と同じ信頼境界にある。ファイルを読める者は所有者になれる。
- 所有者資格はノードローカルで、クラスタの複数ノードをまたぐ「クラスタ所有者」は存在しない。ノード追加・grant発行・回復の承認は、その時点で操作しているノードの管理者に依存する。
- 秘密の保管先がファイルなので、バックアップ・端末交換・クラウド同期・マルウェアの影響を受けやすい。

## 目標

1. 所有者の承認を、特定のノードのファイルではなく、クラスタのポリシーと所有者鍵で表す。
2. 秘密を平文ファイルやURLに置かず、OSの鍵保管（Keychain、Credential Manager、TPM、Secure Enclave等）またはPasskey/WebAuthnへ委ねる。
3. どの信頼済みノードからでも承認でき、オフラインのノードが後から承認結果を検証できる。
4. 鍵の紛失・端末交換・複数所有者・回復を、明示的なポリシーで扱う。
5. 管理UIの操作（grant変更、Provider承認、接続承認、ノード参加・退役）を、所有者の署名またはWebAuthn assertionへ結び付ける。

## 候補設計

### A. Node OwnerとCluster Ownerを分ける

- **Node Owner**: そのノードのOSユーザー。ローカルbootstrap、初期設定、ノード固有の回復に使う。
- **Cluster Owner**: クラスタのroot/admissionに記録する所有者ポリシー。ノード追加、grantの上限、Provider名の事前許可、回復を承認する。
- ノードはCluster Ownerの証明書・公開鍵・ポリシーをrootから検証し、ローカル操作がクラスタポリシーに反しないことを検査する。
- Node OwnerはCluster Ownerの代理になれない。Node Ownerだけではクラスタ全体の管理操作を完了できない。

### B. Owner Key + challenge署名

- 所有者は秘密鍵をOSの鍵保管またはハードウェアに生成し、公開鍵だけをクラスタへ登録する。
- 管理UI/CLIは、操作内容（operation、対象、nonce、期限、ノードID、クラスタID）を正規化したchallengeへ署名する。
- ノードはchallengeのfreshness、単回性、対象、現在のポリシーを検査してから操作を実行する。
- 署名は操作ログにも保存し、後から検証できるようにする。秘密鍵・署名対象の生体情報はログへ出さない。
- ブラウザではWebAuthn/Passkeyのassertionを使い、秘密鍵をブラウザ外へ出さない。CLIではOS鍵保管の鍵を使う。

### C. Owner Policy

rootまたはadmissionに、次のようなポリシーを署名付きで持つ。

```json
{
  "version": 1,
  "owners": [{"key": "hex", "label": "primary"}],
  "threshold": 1,
  "recovery": [{"key": "hex", "label": "backup"}],
  "provider_names": ["ledger/balance"],
  "grant_ceiling": {"read": "all", "write": "all", "call": true, "provide": true}
}
```

- 所有者の追加・削除、threshold変更、回復鍵の変更は、現在のポリシーに従った署名を必要とする。
- 複数所有者・threshold署名を許す場合、管理UIは複数の署名を集める。v1の単一所有者から段階的に移行する。
- ポリシーはノード間で同期し、各ノードがrootからの証明列で検証する。ポリシーを同期しないノードは管理操作をfail closedにする。

### D. Owner Agent / 端末統合

- 常駐するOwner Agentが、OSの生体認証・画面ロック・鍵保管を仲介する。管理UIはAgentへ操作を渡し、Agentが署名済みの承認を返す。
- AndroidはKeystore/StrongBox、iOS/macOSはSecure Enclave/Keychain、WindowsはTPM/Credential Manager、LinuxはSecret Service/TPMを想定する。
- Agentが無い環境では、CLIがOS鍵保管の鍵で署名する。ファイルに秘密鍵を置く経路は緊急回復だけに限定し、明示的な警告を出す。

## 移行案

1. **Phase 0（現在）**: `admin.secret`、管理UI起動リンク、メモリ内セッション。統合接続の承認もこのセッションで行う。
2. **Phase 1**: ノードローカルのOwner KeyをOS鍵保管へ移し、管理UIの操作をchallenge署名またはWebAuthnにする。`admin.secret`はbootstrapと回復だけに残す。
3. **Phase 2**: Cluster Ownerポリシーをroot/admissionへ追加し、ノード追加・grant上限・Provider名の事前許可を署名付き承認にする。管理UIはCluster Ownerの承認を表示する。
4. **Phase 3**: 複数所有者・threshold・回復鍵・鍵ローテーションを追加する。監査ログとバックアップを整備する。

## セキュリティ上の検討点

- challengeのdomain separation（クラスタID、ノードID、操作種別、期限、nonce）と、単回性のdurable保存。
- リプレイ、クロスノード再利用、別クラスタへの流用、管理UIのOrigin偽装。
- WebAuthnのRP IDとloopback originの扱い。ブラウザごとのPasskey同期の可否。
- 鍵紛失時の回復と、回復経路自体の乗っ取り。回復を単一のバックアップファイルにしない。
- 所有者の退職・端末交換・複数端末。ポリシー変更の伝播と、分断中の承認の競合。
- 管理UIのセッションと署名の二重化。セッションだけでは管理操作を完結させない。
- Provider名の事前許可とgrant上限を、所有者ポリシーでどこまで強制するか。
- Cloudflare/Worker配置ではOS鍵保管が使えないため、ホストのsecret managerやAccessとどう組み合わせるか。

## 決めていないこと

- Cluster Ownerを導入する時期と、Node Ownerとの権限分割。
- 単一所有者から始めるか、最初からthresholdを許すか。
- Passkeyを必須にするか、OS鍵保管のCLI署名も同格にするか。
- Owner Policyをrootへ入れるか、別の管理ログへ入れるか。
- 既存`admin.secret`ノードの移行で、最初のOwner Keyを誰が承認するか。
- 管理UIのセッション期限と、署名の有効期限の関係。

## 参照

- [App APIのOAuth契約](app-api.md#6-oauthとローカル構成)
- [Interpreter v1契約](../../contracts/interpreter-v1.md)
- [実装全体レビュー](../records/implementation-review.md)
