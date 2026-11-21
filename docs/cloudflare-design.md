# Cloudflareノード: 実装に向けた設計案

状態: **高水準の方針は所有者確認済み、具体的なwire・実装詳細は設計中**。Cloudflare Workers／SQLite-backed Durable Objectsを常設レプリカとして優先し、端末OSはLinux → Android → Windowsとする。所有者は初版を基本的に同期専用とし、将来の管理操作を望むこと、HTTPSに加えノード鍵の署名で本人確認し、初期の鍵・クラスタ情報を所有者が明示的に渡す方向を承認した。個々の期限・予算・互換性・秘密保管方式をこの文書だけで確定仕様にしない。HarmionのWorkersノードは2026-11-21時点でローカルworkerdまで実装・検証済みで、実Cloudflareには未配置。ローカル実装・検証済み: 署名HTTPSのframe codec、既知署名identityのpin、HMC v2 `SignedHttps` bindingと登録v4、durable challenge store、nativeのPeer v2入口とURL transport、DOのhost境界とpeer-onlyのnode保存adapter、host専用で冪等なprovisioning、登録コード、読み取り専用のWorker管理API、既存Svelte管理画面のcapability対応、Worker発のoutbound同期とDO alarmスケジューラ、本番デプロイ資産（`harmion-worker`、wasm32 check通過）。**workerd上**: wasm-bindgenのDO bridge（`transactionSync`＋`sql.exec`）、WorkerのPeer／管理API／provisioning／静的UI配線、workerd上でのprovision→署名付き同期→再起動保持。**未実装**: 実Cloudflareの配置・Accessポリシー・secret配布・TLS終端、DO起動時の全履歴検証、管理APIのrate limit。App/Query/OAuth面はpeer-onlyとしてfail closed。詳細は[実装レビュー](implementation-review.md) R33〜R48と[transportのdraft](../contracts/https-peer-v2-draft.md)。

## 1. 変えない前提

- 一人の所有者・一つのクラスタ。Cloudflareはprimaryでも参加資格の中央判定者でもない。
- 一クラスタに一つのDO、その中のHarmion Node ID・ログ世代・鍵は永続化する。cold startのたびに作り直さない。
- Androidは電池・回線制約のある断続接続ノード。CloudflareからAndroidへの着信や常駐を同期の成立条件にしない。
- 端末からのHTTPS push/pullで開始し、常駐タイマー・WSS通知は必須にしない。
- Peerの本人確認と、現在のクラスタ参加資格の検査を両方行う。URL、DOのrouting ID、Origin、自己申告のNode IDだけでは認証しない。
- Peerとしての同期権限と、App APIの管理grantを分ける。nativeの `admin.secret` を遠隔ノードへコピーしない。
- 承認済みノードでCloudflareが本文を読めることは既存仕様で許容されている。独自の保存暗号化を今回の成立条件にしない。

## 2. 先に検証する保存境界

既存のdomain・protocol・nodeは `wasm32-unknown-unknown` 向けcheckが通過した。これはWorkersでの動作やDOのtransactionを保証するものではない。

DOのSQLを、同期的なStore transaction callbackへ接続する必要がある。nativeのrusqlite/Tokio/ソケットをWorkersへ持ち込まない。workers-rsの非同期KV transactionを同等の保存境界とは扱わず、実ランタイムの `storage.transactionSync` を試す。必要なら薄いJS bindingをhostに置く。

検証項目:

1. 正常prefixを書いた後の例外で、レコードとcursorが一緒にrollbackすること。
2. 同じID・同じbytesの再送と、同じID・異なるbytesの拒否を区別できること。
3. SQLiteの64-bit位置・時刻をJS Numberで丸めず、文字列またはbytesで正確に往復できること。
4. BLOBのbyte一致、再起動後のNode ID・ログ世代・保存データの保持。
5. Rustの `Err` をJS callbackの通常の戻り値にせず、rollbackを起こす例外へ変換すること。panic/trapは別途実証すること。

[ローカルSQL probe](../design_checks/worker_sql/README.md) は、固定したworkerdで1〜4を検証した。Error objectを通常の戻り値にするとcommitすることと、`9007199254740993` の通常のJS Number取得が `9007199254740992` に丸まることも実測した。例外・文字列cast・BLOBによる扱いを使い分ける必要がある。JSだけの結果から、5やHarmionのnode use caseの適合性、公開認証を検証済みとは呼ばない。

## 3. HTTP本人確認の方向（高水準は所有者確認済み）

推奨は、通常のHTTPSサーバー証明書検証に加えて、**ノード鍵によるアプリケーション層の署名で相互に本人確認する方式**。証明書検証を無効化せず、署名があればどんなendpointへ資格情報を送ってもよいとはしない。

- 要求と応答を、Cluster ID・送信元Node ID・宛先Node ID・用途・要求内容へ結び付ける。
- 応答を今回の要求に結び付け、過去のACK／cursor応答の再利用で受領確認を捏造できないようにする。
- 再送・リプレイ防止・期限・同時要求数・状態の保存予算は、wire契約とテストで具体化する。時計を更新の因果関係には使わない。
- 本文・method・path・送り先を署名対象から落とさず、別クラスタ・別サービスでの署名流用を防ぐ。
- クラウド側も所有者が確認した安定した公開鍵でNode IDを証明する。CloudflareのTLS終端だけをHarmionノードの証明とはしない。

代案は専用のPeer資格情報やCloudflare側のmTLS等。ただし、クラスタ共通のBearerを全端末で共有する方式にはせず、新ノード参加の伝播・失効・資格情報発行・無料運用の制約を別途検討する。

既存のmembership bindingはnative TLS leafかiroh Endpoint IDだけを扱う。署名付きHTTPSのbindingを追加する場合は、証明・登録・codecの互換性を明示する。旧ノードが未知のbindingを黙って別の意味で受理する変更はしない。

## 4. 初回bootstrapの方向（高水準は所有者確認済み）

- 初期のNode鍵、クラスタ共通の信頼起点、必要な参加情報は、デプロイ所有者がローカルの準備ツールで明示的に用意して渡す。
- 未設定のDOは公開要求から初期化しない。最初にアクセスした相手を所有者や信頼起点にはしない。
- 同じ初期設定の再適用、異なる初期設定の差替え、同時初期化を区別し、既存の鍵・Node IDを暗黙に作り直さない。
- 秘密は公開設定・ログ・URLへ置かず、所有者のデプロイ用秘密保管経路で渡す。
- 古いクラウドバックアップや鍵喪失からの復元も、同じNode ID／ログ位置を使い回すと同期cursorを壊し得る。新しいidentityでの再参加と、運用バックアップの扱いを定める。

## 5. 初版の公開面: Aで先行、Bは将来対応

所有者は基本的に同期専用で先行する方針を確認した。将来の管理操作のため、Peer認証とApp APIの管理資格を分離し、追加可能な構造を維持する。Bを初版に実装済みとは扱わない。

### A. Peer同期専用で先行する案

Contentと署名付き参加・退役・却下証明のpush/pullを提供する。一般データApp APIと遠隔管理App APIは公開しない。管理証明は有効なLinuxノード等から伝播させ、初回bootstrapやクラウドの鍵の設定は所有者の明示的なデプロイ操作に限る。

実装範囲と公開面を小さくできるが、クラウドしか有効ノードが残っていない場合の管理・回復はデプロイ所有者の回復手順が必要になる。これはクラスタ管理証明の同期を省略する案ではない。

### B. 最小限の遠隔管理App APIも初版に含める案

参加状態の確認、追加・退役、保留の確定／却下など必要な操作を、既存と同じApp API・grant検査の中で公開する。Peer本人確認とは別の、ノードローカルの遠隔管理資格情報を必要とする。

管理資格情報の発行・所有者への受渡し・期限・失効・再発行と、Linux管理ツールからの遠隔接続の契約も初版に含む。独自の別Admin APIや、Peer資格から管理grantへの暗黙の昇格は導入しない。

いずれの案も、実際のCloudflareアカウント設定、デプロイ、課金・無料枠の保証、公開ネットワークの設定は別の作業。ローカル実行で通っただけでは運用可能と評価しない。

## Tailnetと入口の追加保護

所有者から、閉じたTailnetだけにWorkersを公開できないかという検討事項が出た。Workersを通常のTailnet端末としてIPで待ち受ける標準構成と、公開HTTPSへ認証を重ねる構成は区別する。

- Workersの公開URLはCloudflare Accessで保護でき、自動クライアントはService AuthのService Token等を使える。ただしこれはTailnet所属そのものの認証ではない。
- Workers VPCはWorkersから私設サービスへ接続するための機能であり、今回のTailnet限定の入口とは通信の向きが異なる。
- Tailnet内ゲートウェイを経由し、Workers側も直接アクセスを拒否する構成は別の運用案。そのゲートウェイの常時稼働が新たな依存になるため、断続接続のAndroidへ無条件に担わせない。
- 初版では常設ゲートウェイを必須にせず、Harmionの署名認証を本体の境界として実装し、Access等は任意の追加保護として設計可能にする。具体的なアカウント設定や追加資格情報の導入は別途所有者が選ぶ。TailnetやAccessを通過しても、現在のクラスタ参加資格の検査は省略しない。

確認した公式資料（2026-11-20）: [Workers公開URLとAccess](https://developers.cloudflare.com/workers/configuration/routing/workers-dev/)、[Access Service Token](https://developers.cloudflare.com/cloudflare-one/access-controls/service-credentials/service-tokens/)、[Workers VPC](https://developers.cloudflare.com/workers-vpc/)。

## 6. 段階的な完了条件

1. ローカルランタイムで保存境界と数値精度を検証する。
2. 本人確認・bootstrap・公開管理範囲を確定し、バージョン付き契約と攻撃的な拒否テストを作る。
3. 必要なDO保存portとPeer入口を実装し、認可・受理・ログ・cursorの原子性を既存nodeの意味で確認する。
4. nativeのHTTPS URL adapterと接続し、cold start・中断・再送・退役・分断後収束を実プロセスで確認する。
5. 所有者が運用構成を確認した後に実配置を検証する。その後AndroidのOSホスト、Windowsの所有者保護・常駐を順に進める。

参照: [仕様](specification.md)、[architecture](architecture.md)、[実装レビュー](implementation-review.md)、[Peer契約](../contracts/peer-v1.md)、[参加証明の契約](../contracts/membership-v1.md)。
