<script lang="ts">
  import { onDestroy } from 'svelte';
  import QRCode from 'qrcode';
  import { api, errorMessage, short } from '../lib/api.svelte';
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
    if (!transfer) {
      qr = '';
      qrError = '';
      return;
    }
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

<div class="page-title">
  <div>
    <p class="eyebrow">PEER CONNECTIONS</p>
    <h1>Peer</h1>
    <p class="muted">
      同じクラスタに参加するノードを追加・確認します。参加資格の状態と、実際に接続できるかは別です。
    </p>
  </div>
  <button disabled={busy} onclick={refresh}>{busy ? '更新中…' : '状態を更新'}</button>
</div>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if message}<p class="success" role="status">{message}</p>{/if}

{#if readOnly}
  <section class="panel">
    <div class="section-head">
      <div>
        <h2>Peer（読み取り専用）</h2>
        <p class="muted small">
          この Worker は Peer 専用レプリカです。参加承認・退役・手動同期はこの画面では提供しません。
        </p>
      </div>
      <span class="badge badge-muted">読み取り専用</span>
    </div>
    {#if (overview?.peers ?? []).length === 0}
      <p class="empty">登録された Peer はありません。</p>
    {:else}
      {#each overview?.peers ?? [] as peer}
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
            <p class="mono muted small">{peer.node_id}</p>
            <p class="small">
              {transportLabel(peer.binding.transport)} · {bindingTarget(peer.binding)}
            </p>
          </div>
        </div>
      {/each}
    {/if}
    <h3>参加状態</h3>
    {#if (overview?.members.members ?? []).length === 0}
      <p class="muted small">参加状態はまだありません。</p>
    {:else}
      {#each overview?.members.members ?? [] as member}
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
            <p class="mono muted small">{member.node_id}</p>
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
        <img class="qr" src={qr} alt="このノードの接続コードを含む QR" width="300" height="300" />
      {/if}
      {#if qrError}<p class="warning">{qrError}</p>{/if}
      {#if overview && overview.peer_transfer}
        <label for="own-code">Peer 接続コード</label>
        <textarea id="own-code" class="code-input" value={overview.peer_transfer} readonly
        ></textarea>
        <Clipboard text={overview.peer_transfer} label="接続コードをコピー" />
      {:else}
        <p class="muted small">
          このノードは現在 Peer 接続コードを発行していません。ノードの identity と Peer
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
            message = '読み取りました。内容の確認へ進んでください。';
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
