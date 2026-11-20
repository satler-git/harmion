<script lang="ts">
  import { onDestroy } from 'svelte';
  import QRCode from 'qrcode';
  import { api, errorMessage, short } from '../lib/api.svelte';
  import type { Overview, PeerPreview, SyncResult } from '../lib/types';
  import Confirm from '../lib/Confirm.svelte';
  import Clipboard from '../lib/Clipboard.svelte';
  import QrScanner from '../lib/QrScanner.svelte';
  let {
    overview,
    refresh,
    active,
  }: { overview: Overview | null; refresh: () => Promise<void>; active: boolean } = $props();
  let input = $state(''),
    mode = $state<'approve' | 'join'>('approve'),
    preview = $state<PeerPreview | null>(null);
  let busy = $state(false),
    error = $state(''),
    message = $state(''),
    qr = $state(''),
    qrError = $state(''),
    confirmNode = $state<string | null>(null);
  let lastSync = $state<Record<string, { time: string; report: SyncResult }>>({});
  let showScanner = $state(false);
  let retirement = $state<{
    certificate: string;
    target: string;
    action: 'confirm_retirement' | 'dismiss_retirement';
  } | null>(null);
  $effect(() => {
    if (!active) showScanner = false;
  });
  let qrGeneration = 0;
  $effect(() => {
    const transfer = overview?.peer_transfer;
    const generation = ++qrGeneration;
    if (transfer)
      QRCode.toDataURL(transfer, { width: 360, margin: 3, errorCorrectionLevel: 'L' })
        .then((url) => {
          if (generation === qrGeneration) {
            qr = url;
            qrError = '';
          }
        })
        .catch(() => {
          if (generation === qrGeneration) {
            qr = '';
            qrError = 'この登録情報は QR の容量を超えています。接続コードをコピーしてください。';
          }
        });
  });
  onDestroy(() => {
    qrGeneration++;
  });
  async function review() {
    busy = true;
    error = '';
    message = '';
    preview = null;
    const submitted = input,
      action = mode;
    try {
      const result = await api<PeerPreview>('/peer-preview', { input: submitted, action });
      if (input === submitted && mode === action) preview = result;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  async function connect() {
    if (!preview) return;
    busy = true;
    error = '';
    try {
      await api('/peer-confirm', { ticket: preview.ticket });
      preview = null;
      input = '';
      message = '接続先の登録を保存しました。相手側でも表示された接続コードを確認してください。';
      await refresh();
    } catch (e) {
      error = errorMessage(e);
      preview = null;
    } finally {
      busy = false;
    }
  }
  async function sync(node: string) {
    busy = true;
    error = '';
    try {
      const report = await api<SyncResult>('/node', { type: 'sync', node });
      lastSync[node] = { report, time: new Date().toLocaleTimeString('ja-JP') };
      message = '手動同期が完了しました。';
      await refresh();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  async function retire() {
    if (!confirmNode) return;
    busy = true;
    error = '';
    try {
      await api('/node', { type: 'retire', node: confirmNode });
      confirmNode = null;
      await refresh();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  async function resolveRetirement() {
    if (!retirement) return;
    busy = true;
    error = '';
    try {
      await api('/node', { type: retirement.action, certificate: retirement.certificate });
      retirement = null;
      await refresh();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="page-title">
  <div>
    <p class="eyebrow">PEER CONNECTIONS</p>
    <h1>Peer</h1>
  </div>
  <button disabled={busy} onclick={refresh}>状態を更新</button>
</div>
{#if error}<p class="error" role="alert">{error}</p>{/if}{#if message}<p
    class="success"
    role="status"
  >
    {message}
  </p>{/if}
<div class="two-columns">
  <section class="panel">
    <h2>このノードを共有</h2>
    <p class="muted">相手の管理画面で読み取るか、接続コードを貼り付けてください。</p>
    {#if qr}<img
        class="qr"
        src={qr}
        alt="このノードの接続コードを含む QR"
        width="300"
        height="300"
      />{/if}
    {#if qrError}<p class="warning">{qrError}</p>{/if}
    {#if overview}<label for="own-code">Peer 接続コード</label><textarea
        id="own-code"
        class="code-input"
        value={overview.peer_transfer}
        readonly></textarea><Clipboard
        text={overview.peer_transfer}
        label="接続コードをコピー"
      />{/if}
  </section>
  <section class="panel">
    <h2>Peer を追加</h2>
    <label for="peer-code">相手の接続コード</label><textarea
      id="peer-code"
      class="code-input"
      bind:value={input}
      oninput={() => (preview = null)}
      placeholder="harmion-peer:v1:…"></textarea>
    <button
      onclick={() => {
        showScanner = !showScanner;
      }}
    >
      QR の読み取り{showScanner ? 'を閉じる' : ''}
    </button>
    {#if showScanner}<QrScanner
        onread={(text) => {
          input = text;
          preview = null;
          showScanner = false;
          message = '読み取りました。内容の確認へ進んでください。';
        }}
      />{/if}
    <label for="peer-action">このノードで行う操作</label><select
      id="peer-action"
      bind:value={mode}
      onchange={() => (preview = null)}
    >
      <option value="approve">相手の参加を承認する</option><option value="join">
        このノードを相手のクラスタに参加させる
      </option>
    </select>
    <p class="muted small">
      相手と同じ Cluster ID
      のノードを初期化して使います。既存ノードで新ノードを承認し、新ノードで「参加させる」を実行します。
    </p>
    <button class="primary" disabled={busy || !input.trim()} onclick={review}>
      接続情報を確認
    </button>
    {#if preview}<Confirm
        title="この Peer を登録しますか？"
        {busy}
        onsubmit={connect}
        oncancel={() => (preview = null)}
      >
        <dl>
          <dt>操作</dt>
          <dd>{preview.action === 'approve' ? '相手の参加を承認' : 'このノードを参加させる'}</dd>
          <dt>Cluster ID</dt>
          <dd class="mono">{preview.registration.cluster_id}</dd>
          <dt>Node ID</dt>
          <dd class="mono">{preview.registration.node_id}</dd>
          <dt>署名公開鍵</dt>
          <dd class="mono">{preview.registration.signing_key}</dd>
          <dt>通信方式</dt>
          <dd>{preview.registration.binding.transport}</dd>
        </dl>
        <p>
          相手の画面と
          ID・公開鍵を照合してください。承認後、相手はクラスタの全データを複製できます。
        </p>
      </Confirm>{/if}
  </section>
</div>
<section class="panel">
  <h2>参加状態</h2>
  <p class="muted small">
    Active は参加資格の状態です。オンライン状態は手動同期の結果で確認できます。
  </p>
  {#each overview?.members.members ?? [] as peer}
    <div class="peer-row">
      <div>
        <strong>
          {peer.node_id === overview?.identity.node_id ? 'このノード' : short(peer.node_id)}
        </strong>
        <p class="mono muted small">{peer.node_id}</p>
        {#if lastSync[peer.node_id]}<p class="small success">
            同期完了 {lastSync[peer.node_id].time} · 送信 {lastSync[peer.node_id].report.uploaded} / 受信
            {lastSync[peer.node_id].report.downloaded}
          </p>{/if}
      </div>
      <span class="badge">{peer.state}</span>
      {#if peer.node_id !== overview?.identity.node_id && peer.state === 'active'}<div
          class="actions"
        >
          <button disabled={busy} onclick={() => sync(peer.node_id)}>同期</button><button
            class="danger"
            onclick={() => (confirmNode = peer.node_id)}
          >
            退役
          </button>
        </div>{/if}
    </div>
  {/each}
  {#if confirmNode}<Confirm
      title="Peer の退役"
      {busy}
      onsubmit={retire}
      oncancel={() => (confirmNode = null)}
    >
      <p class="mono">{confirmNode}</p>
      <p>この Node ID の参加を取り消します。再参加には新しい Node ID が必要です。</p>
    </Confirm>{/if}
  {#if overview?.members.pending_retirements.length}
    <h3>判断待ちの退役証明</h3>
    {#each overview.members.pending_retirements as pending}
      <div class="peer-row">
        <div>
          <p>対象 Node ID</p>
          <p class="mono small">{pending.target}</p>
          <details>
            <summary>証明と発行元</summary>
            <p class="mono small">{pending.certificate_id}</p>
            <p class="mono small">{pending.issuer}</p>
          </details>
        </div>
        <div class="actions">
          <button
            class="danger"
            disabled={busy}
            onclick={() =>
              (retirement = {
                certificate: pending.certificate_id,
                target: pending.target,
                action: 'confirm_retirement',
              })}
          >
            退役を確定
          </button><button
            disabled={busy}
            onclick={() =>
              (retirement = {
                certificate: pending.certificate_id,
                target: pending.target,
                action: 'dismiss_retirement',
              })}
          >
            証明を却下
          </button>
        </div>
      </div>
    {/each}
  {/if}
  {#if retirement}<Confirm
      title={retirement.action === 'confirm_retirement' ? '退役の確定' : '保留証明の却下'}
      {busy}
      onsubmit={resolveRetirement}
      oncancel={() => (retirement = null)}
    >
      <p class="mono">{retirement.target}</p>
      <p>
        {retirement.action === 'confirm_retirement'
          ? 'このノードの退役を確定します。'
          : '選択した保留中の証明を却下します。'}
      </p>
    </Confirm>{/if}
</section>
