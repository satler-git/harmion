<script lang="ts">
  import { onDestroy } from 'svelte';
  import QRCode from 'qrcode';
  import { api, count, errorMessage, peersOf, short } from '../lib/api.svelte';
  import type { Overview, PeerPreview, SyncResult } from '../lib/types';
  import Confirm from '../lib/Confirm.svelte';
  import Clipboard from '../lib/Clipboard.svelte';
  import QrScanner from '../lib/QrScanner.svelte';
  import Icon from '../lib/Icon.svelte';
  import {
    bindingTarget,
    membershipStateLabel,
    membershipStateTone,
    pinProvenanceLabel,
    shortId,
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
  let sheet = $state(false),
    sheetTab = $state<'invite' | 'join'>('invite'),
    showRetirements = $state(false);
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
  const clusterBinding = $derived(overview?.identity.binding ?? null);
  const nodeRows = $derived(
    workerMembers.map((member) => {
      const peer = workerPeers.find((item) => item.node_id === member.node_id);
      const transport = member.binding?.transport ?? peer?.binding.transport;
      return {
        member,
        self: member.node_id === selfNode,
        transport: transport ? transportLabel(transport) : '—',
        received: peer?.received_through
          ? count(peer.received_through)
          : (lastSync[member.node_id]?.time ?? '—'),
        pin: peer?.pin ? pinProvenanceLabel(peer.pin.provenance) : null,
      };
    }),
  );

  $effect(() => {
    if (!active) {
      showScanner = false;
      qrZoom = false;
      sheet = false;
      showRetirements = false;
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
  function openAdd(tab: 'invite' | 'join' = 'invite') {
    sheetTab = tab;
    sheet = true;
    preview = null;
  }
  function closeSheet() {
    sheet = false;
    preview = null;
    showScanner = false;
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
      sheet = false;
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
    if (event.key === 'Escape') {
      if (qrZoom) qrZoom = false;
      else if (sheet) closeSheet();
    }
  }}
/>

<div class="page-title">
  <div>
    <h1>ノード</h1>
  </div>
  <div class="actions">
    {#if !readOnly}
      <button class="primary" onclick={() => openAdd('invite')}>
        <Icon name="plus" size={15} />追加
      </button>
    {/if}
    <button class="icon-button" aria-label="更新" title="更新" disabled={busy} onclick={refresh}>
      <Icon name="refresh" size={17} />
    </button>
  </div>
</div>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if message}<p class="success" role="status">{message}</p>{/if}

{#if overview}
  {#if workerPending.length}
    <div class="retire-alert">
      <span class="badge badge-warn">退役証明</span>
      <strong>{short(workerPending[0].target)} の退役</strong>
      <span class="muted small">発行元 {short(workerPending[0].issuer)}</span>
      <button class="quiet" onclick={() => (showRetirements = !showRetirements)}>
        {showRetirements ? '閉じる' : '確認'}
      </button>
    </div>
    {#if showRetirements}
      <section class="panel retirement-panel">
        <div class="section-head">
          <div><h2>保留中の退役証明</h2></div>
          <span class="badge badge-warn">{workerPending.length} 件</span>
        </div>
        {#each workerPending as pending (pending.certificate_id)}
          <div class="retirement-row">
            <div>
              <p class="mono small">{pending.target}</p>
              <details>
                <summary>証明と発行元を表示</summary>
                <p class="mono small">証明 ID: {pending.certificate_id}</p>
                <p class="mono small">発行元: {pending.issuer}</p>
              </details>
            </div>
            {#if !readOnly}
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
            {/if}
          </div>
        {/each}
      </section>
    {/if}
  {/if}

  <section class="panel nodes-panel">
    {#if nodeRows.length === 0}
      <p class="empty">参加ノードはありません。</p>
    {:else}
      <div class="data-table nodes-table" role="table" aria-label="ノード一覧">
        <div class="data-head" role="row">
          <span>ノード</span>
          <span>状態</span>
          <span>接続</span>
          <span>最終同期</span>
          <span></span>
        </div>
        {#each nodeRows as row (row.member.node_id)}
          <div class="data-row" role="row">
            <div>
              <div class="node-name">
                <strong>{row.self ? 'このノード' : short(row.member.node_id)}</strong>
                {#if row.pin}<span class="badge" title={row.pin}>pin</span>{/if}
              </div>
              <span class="mono row-sub" title={row.member.node_id}>
                {shortId(row.member.node_id, 14, 8)}
              </span>
            </div>
            <span class="badge {membershipStateTone(row.member.state)}">
              {membershipStateLabel(row.member.state)}
            </span>
            <span class="muted small">{row.transport}</span>
            <span class="muted small">{row.received}</span>
            <div class="row-actions">
              {#if !row.self && !readOnly && row.member.state === 'active'}
                <button
                  class="quiet small-button"
                  disabled={busy}
                  onclick={() => sync(row.member.node_id)}
                >
                  同期
                </button>
              {/if}
              {#if !readOnly && !row.self && row.member.state === 'active'}
                <details class="row-menu">
                  <summary aria-label="操作"><Icon name="dots" size={16} /></summary>
                  <div class="row-menu-body">
                    <Clipboard text={row.member.node_id} label="IDをコピー" />
                    <button
                      class="danger"
                      disabled={busy}
                      onclick={() => (confirmNode = row.member.node_id)}
                    >
                      退役
                    </button>
                  </div>
                </details>
              {:else}
                <Clipboard text={row.member.node_id} label="IDをコピー" iconOnly />
              {/if}
            </div>
          </div>
        {/each}
      </div>
    {/if}
  </section>

  <section class="panel cluster-strip">
    <div>
      <span class="meta-label">Cluster ID</span>
      <span class="cluster-value mono">
        {shortId(overview.identity.cluster_id, 14, 8)}
        <Clipboard text={overview.identity.cluster_id} label="Cluster ID をコピー" iconOnly />
      </span>
    </div>
    <div>
      <span class="meta-label">接続</span>
      <span class="cluster-value">
        {clusterBinding ? transportLabel(clusterBinding.transport) : '—'}
      </span>
    </div>
    <div>
      <span class="meta-label">次回同期</span>
      <span class="cluster-value">{readOnly ? '—' : '手動'}</span>
    </div>
  </section>
{:else if busy}
  <p class="empty">ノードの状態を取得しています…</p>
{/if}

{#if sheet && !readOnly}
  <div class="sheet-overlay" role="presentation" onclick={closeSheet}></div>
  <div class="sheet-panel" role="dialog" aria-modal="true" aria-labelledby="add-node-title">
    <div class="sheet-head">
      <h2 id="add-node-title">ノードを追加</h2>
      <button class="icon-button" aria-label="閉じる" title="閉じる" onclick={closeSheet}>
        <Icon name="close" size={17} />
      </button>
    </div>
    <div class="segmented sheet-tabs" role="tablist" aria-label="ノードの追加方法">
      <button
        role="tab"
        aria-selected={sheetTab === 'invite'}
        class:active={sheetTab === 'invite'}
        onclick={() => (sheetTab = 'invite')}
      >
        招待
      </button>
      <button
        role="tab"
        aria-selected={sheetTab === 'join'}
        class:active={sheetTab === 'join'}
        onclick={() => (sheetTab = 'join')}
      >
        参加
      </button>
    </div>

    {#if sheetTab === 'invite'}
      <div class="sheet-body">
        {#if qr}
          <button
            class="qr-button sheet-qr"
            type="button"
            title="QR を拡大"
            onclick={() => (qrZoom = true)}
          >
            <img
              class="qr"
              src={qr}
              alt="このノードの接続コードを含む QR"
              width="240"
              height="240"
            />
          </button>
        {:else if qrError}
          <p class="warning">{qrError}</p>
        {/if}
        <span class="meta-label">接続コード</span>
        {#if overview?.peer_transfer}
          <div class="sheet-code mono">{overview.peer_transfer}</div>
          <div class="sheet-actions">
            <Clipboard text={overview.peer_transfer} label="コピー" />
            {#if qr}<a class="button" href={qr} download="harmion-peer.png">QR を保存</a>{/if}
          </div>
        {:else}
          <p class="muted small">接続コードを発行していません。</p>
        {/if}
      </div>
    {:else}
      <div class="sheet-body">
        <label for="peer-code">接続コード</label>
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
            <p>相手をこのクラスタへ追加します。</p>
          {:else}
            <p>参加先で先に承認されたコードを使います。</p>
          {/if}
        </div>
        <button class="primary" disabled={busy || !input.trim()} onclick={review}>
          接続情報を確認
        </button>

        {#if preview}
          <Confirm
            title={preview.action === 'approve'
              ? 'このノードを参加させますか？'
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
              Cluster ID・Node ID・公開鍵を照合してください。承認後、相手は全データを複製できます。
            </p>
          </Confirm>
        {/if}
      </div>
    {/if}
  </div>
{/if}

{#if qrZoom && qr}
  <div class="qr-overlay" role="dialog" aria-modal="true" aria-label="接続コードの QR">
    <button class="qr-close" type="button" aria-label="閉じる" onclick={() => (qrZoom = false)}>
      <Icon name="close" size={18} />
    </button>
    <img src={qr} alt="拡大した接続コードの QR" />
    <button type="button" onclick={() => (qrZoom = false)}>閉じる</button>
  </div>
{/if}

{#if confirmNode}
  <Confirm title="ノードの退役" {busy} onsubmit={retire} oncancel={() => (confirmNode = null)}>
    <p class="mono">{confirmNode}</p>
    <p>この Node ID の参加を取り消します。再参加には新しい Node ID が必要です。</p>
  </Confirm>
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

<style>
  .retire-alert {
    display: flex;
    align-items: center;
    gap: 0.55rem;
    flex-wrap: wrap;
    border: 1px solid #3a3020;
    background: #1b1810;
    border-radius: 8px;
    padding: 0.6rem 0.8rem;
    margin-bottom: 0.9rem;
  }
  .retire-alert strong {
    font-size: 0.86rem;
    font-weight: 600;
  }
  .retire-alert button {
    margin-left: auto;
  }
  .retirement-panel {
    padding: 1rem 1.2rem;
    margin-bottom: 1rem;
  }
  .retirement-row {
    display: flex;
    align-items: start;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.7rem 0;
    border-top: 1px solid var(--line);
    flex-wrap: wrap;
  }
  .retirement-row:first-of-type {
    border-top: 0;
  }
  .nodes-panel {
    padding: 0.8rem 1.4rem;
  }
  .nodes-table .data-head,
  .nodes-table .data-row {
    grid-template-columns: minmax(10rem, 1.6fr) 5rem 7rem 7rem 7.5rem;
  }
  .node-name {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;
  }
  .node-name strong {
    font-size: 0.88rem;
    font-weight: 600;
  }
  .row-sub {
    display: block;
    color: #7e8ba0;
    font-size: 0.7rem;
    margin-top: 0.15rem;
  }
  .row-actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 0.25rem;
    position: relative;
  }
  .small-button {
    padding: 0.28rem 0.55rem;
    font-size: 0.74rem;
  }
  .row-menu {
    position: relative;
  }
  .row-menu summary {
    list-style: none;
    display: grid;
    place-items: center;
    width: 1.65rem;
    height: 1.65rem;
    border-radius: 6px;
    color: #a9b5c8;
    cursor: pointer;
  }
  .row-menu summary::-webkit-details-marker {
    display: none;
  }
  .row-menu[open] summary,
  .row-menu summary:hover {
    background: #1b2331;
  }
  .row-menu-body {
    position: absolute;
    right: 0;
    top: 1.9rem;
    z-index: 8;
    display: grid;
    gap: 0.3rem;
    min-width: 9rem;
    padding: 0.4rem;
    background: #151b27;
    border: 1px solid #303b4f;
    border-radius: 8px;
    box-shadow: 0 10px 30px #05070b;
  }
  .row-menu-body :global(button) {
    width: 100%;
    justify-content: flex-start;
    padding: 0.38rem 0.5rem;
    font-size: 0.76rem;
  }
  .cluster-strip {
    display: grid;
    grid-template-columns: 1.2fr 1fr 1fr;
    gap: 1rem;
    padding: 1rem 1.2rem;
  }
  .cluster-value {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    margin-top: 0.2rem;
    font-size: 0.82rem;
    min-width: 0;
  }
  .cluster-value.mono {
    overflow-wrap: anywhere;
  }
  .cluster-value :global(.icon-button) {
    width: 1.5rem;
    height: 1.5rem;
    flex-shrink: 0;
  }
  .sheet-overlay {
    position: fixed;
    inset: 0;
    background: rgba(4, 6, 10, 0.66);
    z-index: 40;
  }
  .sheet-panel {
    position: fixed;
    top: 0;
    right: 0;
    bottom: 0;
    width: min(430px, 100vw);
    background: #10141d;
    border-left: 1px solid #2a3343;
    box-shadow: -18px 0 40px rgba(0, 0, 0, 0.35);
    z-index: 41;
    padding: 1.2rem 1.4rem 2rem;
    overflow-y: auto;
  }
  .sheet-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    margin-bottom: 1.1rem;
  }
  .sheet-head h2 {
    margin: 0;
    font-size: 1.1rem;
    font-weight: 600;
  }
  .sheet-tabs {
    width: 100%;
    margin: 0 0 1rem;
  }
  .sheet-tabs button {
    flex: 1;
  }
  .sheet-body :global(textarea) {
    min-height: 6rem;
  }
  .sheet-qr {
    background: #fff;
    border: 0;
    padding: 0.5rem;
    display: block;
    margin: 0 auto 0.9rem;
    width: fit-content;
    border-radius: 8px;
  }
  .sheet-qr .qr {
    display: block;
    width: 240px;
    height: 240px;
  }
  .sheet-code {
    margin: 0.25rem 0 0.7rem;
    padding: 0.6rem 0.7rem;
    background: #0b111a;
    border: 1px solid #354055;
    border-radius: 6px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: 0.7rem;
    line-height: 1.8;
    color: #b8c5dc;
  }
  .sheet-actions {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .sheet-panel :global(.hint-box) {
    margin: 0.7rem 0;
  }
  .sheet-panel :global(.confirmation) {
    margin-top: 1rem;
  }
  @media (max-width: 750px) {
    .nodes-table .data-head,
    .nodes-table .data-row {
      grid-template-columns: minmax(8rem, 1.5fr) 4.5rem 1fr 3.2rem;
    }
    .nodes-table .data-head > :nth-child(4),
    .nodes-table .data-row > :nth-child(4) {
      display: none;
    }
    .cluster-strip {
      grid-template-columns: 1fr;
    }
  }
  @media (max-width: 520px) {
    .sheet-panel {
      padding: 1rem;
    }
    .sheet-qr .qr {
      width: 200px;
      height: 200px;
    }
  }
</style>
