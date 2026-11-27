<script lang="ts">
  import { onDestroy } from 'svelte';
  import QRCode from 'qrcode';
  import { api, count, errorMessage, peersOf, short } from '../lib/api.svelte';
  import type { Overview, PeerPreview, SyncResult } from '../lib/types';
  import Confirm from '../lib/Confirm.svelte';
  import Clipboard from '../lib/Clipboard.svelte';
  import QrScanner from '../lib/QrScanner.svelte';
  import {
    bindingTarget,
    membershipStateLabel,
    membershipStateTone,
    pinProvenanceLabel,
    transportLabel,
  } from '../lib/format';

  let {
    overview,
    refresh,
    active,
    readOnly,
  }: {
    overview: Overview | null;
    refresh: () => Promise<void>;
    active: boolean;
    readOnly: boolean;
  } = $props();

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
  let qrZoom = $state(false);
  let retirement = $state<{
    certificate: string;
    target: string;
    action: 'confirm_retirement' | 'dismiss_retirement';
  } | null>(null);
  const workerPeers = $derived(peersOf(overview));
  const selfNode = $derived(overview?.identity.node_id ?? '');
  const workerMembers = $derived(
    [...(overview?.members.members ?? [])].sort(
      (left, right) => Number(right.node_id === selfNode) - Number(left.node_id === selfNode),
    ),
  );
  const workerPending = $derived(overview?.members.pending_retirements ?? []);
  const workerActive = $derived(workerMembers.filter((member) => member.state === 'active').length);

  $effect(() => {
    if (!active) {
      showScanner = false;
      qrZoom = false;
    }
  });
  let qrGeneration = 0;
  $effect(() => {
    const transfer = overview?.peer_transfer;
    const generation = ++qrGeneration;
    if (!transfer) {
      qr = '';
      qrError = '';
      return;
    }
    // Render at a high resolution and keep the full four-module quiet zone.
    // The image is scaled down for the sidebar and shown nearly 1:1 in the
    // enlarge overlay; small or tightly cropped QR codes are hard to read from
    // a phone camera, especially on a dark page.
    QRCode.toDataURL(transfer, { width: 960, margin: 4, errorCorrectionLevel: 'L' })
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
    message = '';
    try {
      await api('/node', { type: 'retire', node: confirmNode });
      const retired = confirmNode;
      confirmNode = null;
      message = `ノード ${short(retired)} の退役を反映しました。`;
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

<svelte:window
  onkeydown={(event) => {
    if (event.key === 'Escape' && qrZoom) qrZoom = false;
  }}
/>

<div class="page-title">
  <div>
    <h1>Peer</h1>
  </div>
  <button disabled={busy} onclick={refresh}>{busy ? '更新中…' : '状態を更新'}</button>
</div>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if message}<p class="success" role="status">{message}</p>{/if}

{#if readOnly}
  <section class="panel" aria-label="Peer の状態">
    <div class="worker-summary">
      <span>参加 {workerActive} / {workerMembers.length}</span>
      <span>Peer {workerPeers.length}</span>
      <span>pin {workerPeers.filter((peer) => peer.pinned).length}</span>
      {#if workerPending.length}<span class="badge badge-warn">
          退役証明 {workerPending.length}
        </span>{/if}
    </div>

    <h2>メンバー</h2>
    {#if workerMembers.length === 0}
      <p class="empty">参加状態はまだありません。</p>
    {:else}
      {#each workerMembers as member}
        <div class="peer-row">
          <div>
            <div class="title-line">
              <strong>
                {member.node_id === overview?.identity.node_id
                  ? 'このノード'
                  : short(member.node_id)}
              </strong>
              <span class="badge {membershipStateTone(member.state)}">
                {membershipStateLabel(member.state)}
              </span>
            </div>
            <div class="id-line">
              <p class="mono muted small">{member.node_id}</p>
              <Clipboard text={member.node_id} label="コピー" />
            </div>
            {#if member.binding}
              <p class="small">
                {transportLabel(member.binding.transport)} · {bindingTarget(member.binding)}
              </p>
            {/if}
            {#if member.signing_key}
              <p class="mono muted small">公開鍵 {short(member.signing_key)}</p>
            {/if}
          </div>
        </div>
      {/each}
    {/if}

    <h2>Peer 接続</h2>
    {#if workerPeers.length === 0}
      <p class="empty">登録された Peer はありません。</p>
    {:else}
      {#each workerPeers as peer}
        <div class="peer-row">
          <div>
            <div class="title-line">
              <strong>{short(peer.node_id)}</strong>
              <span class="badge {peer.retired ? 'badge-muted' : 'badge-ok'}">
                {peer.retired ? '退役' : '有効'}
              </span>
              {#if peer.pin}
                <span class="badge">pin: {pinProvenanceLabel(peer.pin.provenance)}</span>
              {:else}
                <span class="badge badge-warn">未 pin</span>
              {/if}
            </div>
            <div class="id-line">
              <p class="mono muted small">{peer.node_id}</p>
              <Clipboard text={peer.node_id} label="コピー" />
            </div>
            <p class="small">
              {transportLabel(peer.binding.transport)} · {bindingTarget(peer.binding)}
            </p>
            <p class="muted small">
              更新 {peer.received_through ? count(peer.received_through) : '—'} · 参加 {peer.membership_received_through
                ? count(peer.membership_received_through)
                : '—'}
            </p>
          </div>
        </div>
      {/each}
    {/if}

    {#if workerPending.length}
      <h2>保留中の退役証明</h2>
      <p class="muted small">確定・却下はローカルの管理画面で行います。</p>
      {#each workerPending as pending}
        <div class="peer-row">
          <div>
            <p class="meta-label">対象 Node ID</p>
            <div class="id-line">
              <p class="mono small">{pending.target}</p>
              <Clipboard text={pending.target} label="コピー" />
            </div>
            <details>
              <summary>証明と発行元を表示</summary>
              <p class="mono small">証明 ID: {pending.certificate_id}</p>
              <p class="mono small">発行元: {pending.issuer}</p>
            </details>
          </div>
        </div>
      {/each}
    {/if}
  </section>
{:else}
  <div class="two-columns">
    <section class="panel">
      <h2>このノードを共有</h2>
      <p class="muted">
        相手の管理画面で読み取るか、接続コードを貼り付けてください。参加の向きによって使う操作が異なります。
      </p>
      {#if qr}
        <button class="qr-button" type="button" title="QR を拡大" onclick={() => (qrZoom = true)}>
          <img class="qr" src={qr} alt="このノードの接続コードを含む QR" width="360" height="360" />
        </button>
        <div class="qr-actions">
          <button type="button" onclick={() => (qrZoom = true)}>QR を拡大</button>
          <a class="button" href={qr} download="harmion-peer.png">QR を保存</a>
        </div>
        <p class="muted small">
          Android のカメラで読み取れない場合は、下の接続コードをコピーして貼り付けてください。
        </p>
      {/if}
      {#if qrError}<p class="warning">{qrError}</p>{/if}
      {#if overview && overview.peer_transfer}
        <label for="own-code">Peer 接続コード</label>
        <textarea id="own-code" class="code-input" value={overview.peer_transfer} readonly
        ></textarea>
        <Clipboard text={overview.peer_transfer} label="接続コードをコピー" />
      {:else}
        <p class="muted small">
          このノードは現在 Peer 接続コードを発行していません。ノードの識別情報と Peer
          通信設定を確認してください。
        </p>
      {/if}
    </section>

    <section class="panel">
      <h2>Peer を追加</h2>
      <p class="muted">
        相手から受け取った接続コードを貼り付けます。操作の向きを選んでから内容を確認してください。
      </p>
      <label for="peer-code">相手の接続コード</label>
      <textarea
        id="peer-code"
        class="code-input"
        bind:value={input}
        oninput={() => (preview = null)}
        placeholder="harmion-peer:v1:…"></textarea>
      <div class="actions">
        <button onclick={() => (showScanner = !showScanner)}>
          QR の読み取り{showScanner ? 'を閉じる' : ''}
        </button>
      </div>
      {#if showScanner}
        <QrScanner
          onread={(text) => {
            input = text;
            preview = null;
            showScanner = false;
          }}
        />
      {/if}
      <label for="peer-action">このノードで行う操作</label>
      <select id="peer-action" bind:value={mode} onchange={() => (preview = null)}>
        <option value="approve">相手の参加を承認する</option>
        <option value="join">このノードを相手のクラスタに参加させる</option>
      </select>
      <div class="hint-box">
        {#if mode === 'approve'}
          <p>
            相手のコードを貼り替えると、相手ノードをこのクラスタへ迎え入れます。相手がまだ
            このクラスタに参加していない場合に使います。
          </p>
        {:else}
          <p>
            このノードを相手のクラスタへ参加させます。参加先の既存ノードで先に承認されたコードを
            使ってください。
          </p>
        {/if}
      </div>
      <button class="primary" disabled={busy || !input.trim()} onclick={review}>
        接続情報を確認
      </button>

      {#if preview}
        <Confirm
          title={preview.action === 'approve'
            ? 'この Peer を参加させますか？'
            : 'このクラスタへ参加しますか？'}
          {busy}
          onsubmit={connect}
          oncancel={() => (preview = null)}
        >
          <dl>
            <dt>操作</dt>
            <dd>
              {preview.action === 'approve'
                ? '相手の参加を承認'
                : 'このノードを相手のクラスタへ参加'}
            </dd>
            <dt>Cluster ID</dt>
            <dd class="mono">{preview.registration.cluster_id}</dd>
            <dt>Node ID</dt>
            <dd class="mono">{preview.registration.node_id}</dd>
            <dt>署名公開鍵</dt>
            <dd class="mono">{preview.registration.signing_key ?? '—'}</dd>
            <dt>通信方式</dt>
            <dd>
              {transportLabel(preview.registration.binding.transport)} ·
              {bindingTarget(preview.registration.binding)}
            </dd>
          </dl>
          <p>
            相手の画面と Cluster ID・Node ID・公開鍵を照合してください。承認後、相手はクラスタの
            全データを複製できます。
          </p>
        </Confirm>
      {/if}
    </section>
  </div>

  {#if qrZoom && qr}
    <div class="qr-overlay" role="dialog" aria-modal="true" aria-label="接続コードの QR">
      <button class="qr-close" type="button" aria-label="閉じる" onclick={() => (qrZoom = false)}>
        ×
      </button>
      <img src={qr} alt="拡大した接続コードの QR" />
      <button type="button" onclick={() => (qrZoom = false)}>閉じる</button>
    </div>
  {/if}

  <section class="panel">
    <div class="section-head">
      <div>
        <h2>参加状態</h2>
        <p class="muted small">
          有効は参加資格がある状態です。オンラインかどうかは手動同期の結果で確認できます。
        </p>
      </div>
      <span class="badge badge-ok">
        {overview?.members.members.filter((member) => member.state === 'active').length ?? 0} 台が有効
      </span>
    </div>
    {#if (overview?.members.members ?? []).length === 0}
      <p class="empty">参加ノードはありません。</p>
    {:else}
      {#each overview?.members.members ?? [] as peer}
        <div class="peer-row">
          <div>
            <div class="title-line">
              <strong>
                {peer.node_id === overview?.identity.node_id ? 'このノード' : short(peer.node_id)}
              </strong>
              <span class="badge {membershipStateTone(peer.state)}">
                {membershipStateLabel(peer.state)}
              </span>
            </div>
            <p class="mono muted small">{peer.node_id}</p>
            {#if lastSync[peer.node_id]}
              <p class="small success">
                同期完了 {lastSync[peer.node_id].time} · 送信 {lastSync[peer.node_id].report
                  .uploaded} / 受信 {lastSync[peer.node_id].report.downloaded}
              </p>
            {/if}
          </div>
          {#if peer.node_id !== overview?.identity.node_id && peer.state === 'active'}
            <div class="actions">
              <button disabled={busy} onclick={() => sync(peer.node_id)}>同期</button>
              <button class="danger" disabled={busy} onclick={() => (confirmNode = peer.node_id)}>
                退役
              </button>
            </div>
          {/if}
        </div>
      {/each}
    {/if}

    {#if confirmNode}
      <Confirm title="Peer の退役" {busy} onsubmit={retire} oncancel={() => (confirmNode = null)}>
        <p class="mono">{confirmNode}</p>
        <p>この Node ID の参加を取り消します。再参加には新しい Node ID が必要です。</p>
      </Confirm>
    {/if}

    {#if overview && overview.members.pending_retirements.length}
      <h3>判断待ちの退役証明</h3>
      <p class="muted small">
        他のノードが発行した退役証明です。対象と発行元を確認して、確定または却下してください。
      </p>
      {#each overview.members.pending_retirements as pending}
        <div class="peer-row">
          <div>
            <p class="meta-label">対象 Node ID</p>
            <p class="mono small">{pending.target}</p>
            <details>
              <summary>証明と発行元を表示</summary>
              <p class="mono small">証明 ID: {pending.certificate_id}</p>
              <p class="mono small">発行元: {pending.issuer}</p>
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
            </button>
            <button
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

    {#if retirement}
      <Confirm
        title={retirement.action === 'confirm_retirement' ? '退役の確定' : '保留証明の却下'}
        {busy}
        danger={retirement.action === 'confirm_retirement'}
        onsubmit={resolveRetirement}
        oncancel={() => (retirement = null)}
      >
        <p class="mono">{retirement.target}</p>
        <p>
          {retirement.action === 'confirm_retirement'
            ? 'このノードの退役を確定します。以降、このノードは参加者として扱われません。'
            : '選択した保留中の証明を却下します。'}
        </p>
      </Confirm>
    {/if}
  </section>
{/if}

<style>
  .title-line {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .title-line strong {
    font-size: 0.92rem;
  }
  .peer-row p {
    margin: 0.25rem 0;
  }
  .id-line {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .id-line p {
    margin: 0;
    min-width: 0;
  }
  .id-line :global(button) {
    padding: 0.15rem 0.45rem;
    font-size: 0.72rem;
    flex-shrink: 0;
  }
  .worker-summary {
    display: flex;
    gap: 1.2rem;
    flex-wrap: wrap;
    color: var(--muted);
    font-size: 0.82rem;
    padding: 0.6rem 0;
    border-bottom: 1px solid var(--line);
  }
  .hint-box {
    border: 1px solid #2a3448;
    border-radius: 7px;
    background: #0e131c;
    padding: 0.65rem 0.8rem;
    margin: 0.7rem 0;
  }
  .hint-box p {
    margin: 0;
    color: var(--muted);
    font-size: 0.82rem;
  }
  .panel details {
    margin-top: 0.4rem;
  }
  .panel summary {
    color: var(--accent);
    cursor: pointer;
    font-size: 0.8rem;
  }
</style>
