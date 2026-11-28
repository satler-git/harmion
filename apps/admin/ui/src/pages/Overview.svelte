<script lang="ts">
  import { api, count, peersOf, short, workerAdmin } from '../lib/api.svelte';
  import type {
    Capabilities,
    ConnectionList,
    ConnectionRequest,
    InterpreterProvider,
    Overview,
    Page,
  } from '../lib/types';
  import Clipboard from '../lib/Clipboard.svelte';
  import Grants from '../lib/Grants.svelte';
  import Icon from '../lib/Icon.svelte';
  import WorkerRegistration from '../lib/WorkerRegistration.svelte';
  import {
    formatDateTime,
    membershipStateLabel,
    membershipStateTone,
    microsDate,
    relativeTime,
    remainingTime,
    shortId,
    transportLabel,
  } from '../lib/format';

  let {
    overview,
    capabilities,
    sampledAt,
    refresh,
    busy,
    error,
    navigate,
    readOnly,
    now,
    active,
    approvalCount = 0,
  }: {
    overview: Overview | null;
    capabilities: Capabilities | null;
    sampledAt: string;
    refresh: () => void;
    busy: boolean;
    error: string;
    navigate: (page: Page) => void;
    readOnly: boolean;
    now: number;
    active: boolean;
    approvalCount?: number;
  } = $props();

  function bytes(value: string) {
    try {
      const n = BigInt(value);
      return n < 1048576n ? count(value) + ' B' : (Number(n / 1024n) / 1024).toFixed(1) + ' MiB';
    } catch {
      return '—';
    }
  }
  const configLabels: Record<string, string> = {
    CLUSTER: 'クラスタID',
    NODE_ID: 'ノードID',
    EPOCH: 'Epoch',
    SEED: '署名シード',
    ORIGIN: '公開URL',
    ADMIN_TOKEN: '管理トークン',
  };
  function configLabel(name: string) {
    return configLabels[name] ?? name;
  }

  const worker = $derived(capabilities?.mode === 'worker');
  const hostStatus = $derived(workerAdmin.status);
  const stats = $derived(overview?.stats ?? null);
  const selfState = $derived(
    overview?.self_state ??
      overview?.members.members.find((member) => member.node_id === overview?.identity.node_id)
        ?.state ??
      'unconfigured',
  );
  const peers = $derived(peersOf(overview));
  const activeMembers = $derived(
    overview?.members.members.filter((member) => member.state === 'active').length ?? 0,
  );
  const retiredMembers = $derived(
    overview?.members.members.filter((member) => member.state === 'retired').length ?? 0,
  );
  const lastInboundAt = $derived(hostStatus?.sync.last_inbound_at ?? null);
  const lastInboundText = $derived(
    !hostStatus ? '—' : lastInboundAt ? relativeTime(lastInboundAt, now) : '未受信',
  );
  const lastInboundDetail = $derived(
    !hostStatus ? '取得できません' : lastInboundAt ? formatDateTime(lastInboundAt) : '受信なし',
  );
  const syncInterval = $derived(Number(hostStatus?.sync.interval_ms ?? '0'));
  const stale = $derived(
    syncInterval > 0 && lastInboundAt
      ? now - Number(lastInboundAt) / 1000 > syncInterval * 2
      : false,
  );
  const syncIntervalText = $derived.by(() => {
    if (!hostStatus) return '—';
    if (!(syncInterval > 0)) return '無効（受信のみ）';
    if (syncInterval < 60000) return `${Math.round(syncInterval / 1000)} 秒間隔`;
    return `${Math.round(syncInterval / 60000)} 分間隔`;
  });
  const nextSyncText = $derived.by(() => {
    if (!worker) return '手動';
    if (!hostStatus) return '—';
    if (hostStatus.sync.alarm_next_at) return relativeTime(hostStatus.sync.alarm_next_at, now);
    return syncInterval > 0 ? '待機' : '無効';
  });
  const configIssues = $derived([
    ...new Set([...(hostStatus?.config.missing ?? []), ...(hostStatus?.config.invalid ?? [])]),
  ]);
  const status = $derived.by((): { tone: string; label: string } | null => {
    if (!worker) return { tone: 'badge-ok', label: '正常' };
    if (!hostStatus) {
      if (workerAdmin.statusCode === 404) return { tone: 'badge-muted', label: '未対応' };
      if (workerAdmin.statusError) return { tone: 'badge-danger', label: '状態取得不可' };
      return { tone: 'badge-muted', label: '確認中' };
    }
    if (!hostStatus.config.ok) return { tone: 'badge-danger', label: '設定不備' };
    if (!overview?.configured || !hostStatus.provisioning.provisioned) {
      return { tone: 'badge-danger', label: '未設定' };
    }
    if (!hostStatus.provisioning.identity_matches) {
      return { tone: 'badge-danger', label: '再登録が必要' };
    }
    if (selfState === 'unconfirmed') return { tone: 'badge-warn', label: '承認待ち' };
    if (selfState === 'retired') return { tone: 'badge-danger', label: '退役' };
    if (selfState === 'conflict') return { tone: 'badge-danger', label: '競合' };
    if (selfState !== 'active') return { tone: 'badge-warn', label: '未設定' };
    if (!lastInboundAt) return { tone: 'badge-info', label: '未受信' };
    if (stale) return { tone: 'badge-warn', label: '受信が古い' };
    return { tone: 'badge-ok', label: '正常' };
  });
  const selfLabel = $derived(
    selfState === 'unconfigured' ? '未設定' : membershipStateLabel(selfState),
  );
  const selfTone = $derived(
    selfState === 'unconfigured' ? 'badge-danger' : membershipStateTone(selfState),
  );
  const registrationNeeded = $derived(
    selfState === 'unconfirmed' ||
      selfState === 'unconfigured' ||
      (hostStatus !== null && !hostStatus.provisioning.identity_matches),
  );
  const registrationActive = $derived(
    selfState === 'active' && hostStatus?.provisioning.identity_matches === true,
  );
  const oauthOrigin = $derived(
    (capabilities as (Capabilities & { oauth_origin?: string | null }) | null)?.oauth_origin ??
      null,
  );
  const binding = $derived(overview?.identity.binding ?? null);
  const bindingDetail = $derived(
    binding
      ? binding.transport === 'signed_https'
        ? binding.origin
        : binding.transport === 'iroh'
          ? (binding.relay_url ?? binding.endpoint_id)
          : binding.endpoint
      : null,
  );

  let connections = $state<ConnectionRequest[]>([]);
  let providers = $state<InterpreterProvider[]>([]);
  let pendingLoaded = $state(false);

  const pendingConnections = $derived(
    connections.filter((request) => {
      if (request.status !== 'pending') return false;
      const expires = microsDate(request.expires_at);
      return expires === null || expires.getTime() > now;
    }),
  );
  const pendingProviders = $derived(providers.filter((provider) => provider.status === 'pending'));
  const connectedCount = $derived(
    connections.filter((request) => request.status === 'approved' || request.status === 'completed')
      .length,
  );
  const attention = $derived([
    ...pendingConnections.map((request) => ({
      key: request.connection_id,
      kind: '接続' as const,
      title: request.name,
      detail: request.web_origin ?? 'CLI / ネイティブアプリ',
      remaining: remainingTime(request.expires_at, now),
    })),
    ...pendingProviders.map((provider) => ({
      key: provider.provider_id,
      kind: 'Provider' as const,
      title: provider.name,
      detail: `契約 v${provider.contract_version}`,
      remaining: provider.connected ? '接続中' : '接続前',
    })),
  ]);
  const nodeRows = $derived(
    [...(overview?.members.members ?? [])]
      .sort(
        (left, right) =>
          Number(right.node_id === overview?.identity.node_id) -
          Number(left.node_id === overview?.identity.node_id),
      )
      .map((member) => {
        const peer = peers.find((item) => item.node_id === member.node_id);
        const transport = member.binding?.transport ?? peer?.binding.transport;
        return {
          member,
          self: member.node_id === overview?.identity.node_id,
          transport: transport ? transportLabel(transport) : '—',
          received: peer?.received_through ? count(peer.received_through) : '—',
        };
      }),
  );
  const metricContent = $derived(
    worker
      ? stats
        ? count(stats.contents)
        : '—'
      : overview?.metrics
        ? count(overview.metrics.visible_contents)
        : '—',
  );
  const metricContentDetail = $derived(
    worker ? '削除込み' : overview?.metrics ? `削除込み ${count(overview.metrics.contents)}` : '—',
  );
  const metricUpdates = $derived(
    worker
      ? stats
        ? count(stats.accepted_updates)
        : '—'
      : overview?.metrics
        ? count(overview.metrics.accepted_updates)
        : '—',
  );
  const metricBytes = $derived(
    worker
      ? stats
        ? bytes(stats.record_bytes)
        : '—'
      : overview?.metrics
        ? bytes(overview.metrics.record_bytes)
        : '—',
  );
  const metricApps = $derived(overview?.metrics ? count(overview.metrics.active_apps) : '—');

  const registrationKey = $derived(
    [
      sampledAt,
      overview?.identity.node_id,
      overview?.identity.signing_key,
      hostStatus?.provisioning.identity_matches,
      bindingDetail,
    ].join('|'),
  );

  async function loadPending() {
    const [connectionList, providerList] = await Promise.all([
      api<ConnectionList>('/connect-list', {}).catch(() => null),
      api<{ providers: InterpreterProvider[] }>('/interpreter-list', {}).catch(() => null),
    ]);
    if (connectionList) connections = connectionList.connections;
    if (providerList) providers = providerList.providers;
    pendingLoaded = true;
  }
  async function refreshAll() {
    await Promise.resolve(refresh());
    await loadPending();
  }
  () => {
    if (active) void loadPending();
  };
</script>

<div class="page-title">
  <div>
    <h1>概要</h1>
  </div>
  <button class="icon-button" aria-label="更新" title="更新" disabled={busy} onclick={refreshAll}>
    <Icon name="refresh" size={17} />
  </button>
</div>

{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if overview}
  <div class="status-strip">
    <span class="badge {status?.tone ?? 'badge-muted'}">{status?.label ?? '確認中'}</span>
    {#if worker}
      <span class="status-item">
        <span class="meta-label">最終受信</span>
        <strong>{lastInboundText}</strong>
        <span class="muted small">{lastInboundDetail}</span>
      </span>
      <span class="status-item">
        <span class="meta-label">次回同期</span>
        <strong>{nextSyncText}</strong>
        <span class="muted small">{syncIntervalText}</span>
      </span>
    {:else}
      <span class="status-item">
        <span class="meta-label">同期</span>
        <strong>手動</strong>
      </span>
    {/if}
    <span class="status-item status-sampled">
      <span class="meta-label">取得</span>
      <strong>{sampledAt || '—'}</strong>
    </span>
    {#if worker && hostStatus?.provisioning.bootstrap_enabled}
      <span class="badge badge-warn">初期設定 受付中</span>
    {/if}
    {#if worker && hostStatus?.provisioning.allow_reprovision}
      <span class="badge badge-warn">再初期化 許可</span>
    {/if}
    {#if configIssues.length}
      <span class="badge badge-danger">設定 {configIssues.length} 件</span>
    {/if}
  </div>

  {#if worker && workerAdmin.statusError && !hostStatus}
    <p class="warning" role="status">
      {workerAdmin.statusCode === 404
        ? 'この Worker は状態取得に対応していません。'
        : '状態を取得できませんでした。'}
    </p>
  {/if}

  <div class="metrics">
    <article class="metric-card">
      <span>ノード</span>
      <strong>{count(String(overview.members.members.length))}</strong>
      <small>有効 {activeMembers} · 退役 {retiredMembers}</small>
    </article>
    <article class="metric-card">
      <span>Content</span>
      <strong>{metricContent}</strong>
      <small>{metricContentDetail}</small>
    </article>
    <article class="metric-card">
      <span>受信した更新</span>
      <strong>{metricUpdates}</strong>
      <small>{metricBytes}</small>
    </article>
    <article class="metric-card">
      <span>アプリ</span>
      <strong>{metricApps}</strong>
      <small>{pendingLoaded ? `接続中 ${connectedCount}` : '—'}</small>
    </article>
  </div>

  {#if !readOnly && attention.length}
    <section class="panel attention-panel">
      <div class="section-head">
        <div>
          <h2>要対応</h2>
        </div>
        <span class="badge badge-warn">{attention.length}</span>
      </div>
      <div class="attention-list">
        {#each attention as item (item.key)}
          <div class="attention-row">
            <span class="badge {item.kind === '接続' ? 'badge-info' : 'badge-ok'}">
              {item.kind}
            </span>
            <strong>{item.title}</strong>
            <span class="muted">{item.detail}</span>
            <span class="attention-when">{item.remaining}</span>
            <button class="quiet" onclick={() => navigate('approvals')}>確認</button>
          </div>
        {/each}
      </div>
    </section>
  {/if}

  {#if worker}
    <div class="two-columns overview-worker">
      {@render identityPanel()}
      <section class="panel">
        <div class="section-head">
          <div>
            <h2>同期</h2>
          </div>
          <span class="badge {lastInboundAt ? 'badge-ok' : 'badge-muted'}">
            {lastInboundAt ? '受信あり' : '未受信'}
          </span>
        </div>
        <dl>
          <dt>最終受信</dt>
          <dd>{lastInboundText}</dd>
          <dt>保持位置</dt>
          <dd class="mono small">{stats ? count(stats.accepted_updates) : '—'}</dd>
          <dt>外部同期</dt>
          <dd>{syncIntervalText}</dd>
          {#if hostStatus?.sync.alarm_next_at}
            <dt>次回</dt>
            <dd>{relativeTime(hostStatus.sync.alarm_next_at, now)}</dd>
          {/if}
          {#if hostStatus?.sync.schedule.length}
            <dt>Worker からの同期</dt>
            <dd class="mono small">
              {#each hostStatus.sync.schedule as entry}
                <span class="sync-entry">
                  {entry.node.slice(0, 10)}… 失敗 {entry.failures}{entry.last_ok === false
                    ? ' · 直近は失敗'
                    : ''}
                </span>
              {/each}
            </dd>
          {/if}
        </dl>
        {#if peers.length}
          <h3>ノードごとの受信位置</h3>
          {#each peers as peer}
            <div class="sync-peer">
              <span class="mono small">{peer.node_id.slice(0, 10)}…</span>
              <span class="muted small">
                更新 {peer.received_through ? count(peer.received_through) : '—'} · 参加 {peer.membership_received_through
                  ? count(peer.membership_received_through)
                  : '—'}
              </span>
            </div>
          {/each}
        {/if}
        <div class="actions">
          <button onclick={() => navigate('peers')}>ノードを表示</button>
        </div>
      </section>
    </div>
  {:else}
    <div class="overview-grid">
      <section class="panel node-table-panel">
        <div class="section-head">
          <div>
            <h2>ノード一覧</h2>
          </div>
          <button class="quiet" onclick={() => navigate('peers')}>すべて表示</button>
        </div>
        {#if nodeRows.length === 0}
          <p class="empty">参加ノードはありません。</p>
        {:else}
          <div class="data-table" role="table" aria-label="ノード一覧">
            <div class="data-head" role="row">
              <span>ノード</span>
              <span>状態</span>
              <span>接続</span>
              <span>受信位置</span>
            </div>
            {#each nodeRows as row (row.member.node_id)}
              <div class="data-row" role="row">
                <div>
                  <strong>{row.self ? 'このノード' : short(row.member.node_id)}</strong>
                  <span class="mono row-sub" title={row.member.node_id}>
                    {short(row.member.node_id)}
                  </span>
                </div>
                <span class="badge {membershipStateTone(row.member.state)}">
                  {membershipStateLabel(row.member.state)}
                </span>
                <span class="muted small">{row.transport}</span>
                <span class="muted small">{row.received}</span>
              </div>
            {/each}
          </div>
        {/if}
      </section>
      {@render identityPanel()}
    </div>
  {/if}

  {#if worker}
    {#if configIssues.length}
      <section class="panel">
        <h2>設定の不足</h2>
        <ul class="issues">
          {#each configIssues as issue}<li>{configLabel(issue)}</li>{/each}
        </ul>
      </section>
    {/if}
    {#key registrationKey}
      {#if registrationNeeded}
        <WorkerRegistration approved={false} />
      {:else if registrationActive}
        <WorkerRegistration approved={true} />
      {/if}
    {/key}
  {/if}
{:else if busy}
  <p class="empty">ノードの状態を取得しています…</p>
{/if}

{#snippet identityPanel()}
  <section class="panel identity-panel">
    <div class="section-head">
      <div>
        <h2>識別情報</h2>
      </div>
    </div>
    <dl class="identity-list">
      <dt>Node ID</dt>
      <dd>
        <span class="mono">{shortId(overview!.identity.node_id, 14, 8)}</span>
        <Clipboard text={overview!.identity.node_id} label="Node ID をコピー" iconOnly />
      </dd>
      <dt>Cluster ID</dt>
      <dd>
        <span class="mono">{shortId(overview!.identity.cluster_id, 14, 8)}</span>
        <Clipboard text={overview!.identity.cluster_id} label="Cluster ID をコピー" iconOnly />
      </dd>
      <dt>接続方式</dt>
      <dd class="identity-stack">
        <span>{binding ? transportLabel(binding.transport) : '—'}</span>
        {#if bindingDetail}<span class="mono muted small binding-detail" title={bindingDetail}>
            {bindingDetail.length > 32 ? shortId(bindingDetail, 18, 10) : bindingDetail}
          </span>{/if}
      </dd>
      {#if overview!.identity.signing_key}
        <dt>公開鍵</dt>
        <dd>
          <span class="mono">{shortId(overview!.identity.signing_key, 10, 6)}</span>
          <Clipboard text={overview!.identity.signing_key} label="公開鍵をコピー" iconOnly />
        </dd>
      {/if}
      {#if worker && overview!.identity.cluster_root}
        <dt>Cluster Root</dt>
        <dd>
          <details>
            <summary class="mono small">{short(overview!.identity.cluster_root)}</summary>
            <p class="mono small">{overview!.identity.cluster_root}</p>
          </details>
        </dd>
      {/if}
      {#if worker}
        <dt>自分の参加状態</dt>
        <dd><span class="badge {selfTone}">{selfLabel}</span></dd>
        <dt>初期設定</dt>
        <dd>
          {overview!.configured ? '済み' : '未設定'}
          {#if hostStatus?.provisioning.provisioned}
            <span
              class="badge {hostStatus.provisioning.identity_matches ? 'badge-ok' : 'badge-danger'}"
            >
              {hostStatus.provisioning.identity_matches ? '設定と一致' : '設定と不一致'}
            </span>
          {/if}
        </dd>
      {/if}
    </dl>
    {#if overview!.permissions}
      <details class="identity-grants">
        <summary>権限</summary>
        <Grants grants={overview!.permissions} dense />
      </details>
    {/if}
  </section>
{/snippet}

<style>
  .status-strip {
    display: flex;
    align-items: center;
    gap: 1.1rem;
    flex-wrap: wrap;
    border: 1px solid #283041;
    border-left: 2px solid #5dd39e;
    border-radius: 10px;
    background: #0f131b;
    padding: 0.7rem 0.9rem;
    margin: 0 0 1rem;
  }
  .status-item {
    display: flex;
    align-items: baseline;
    gap: 0.4rem;
    min-width: 0;
  }
  .status-item strong {
    font-weight: 600;
  }
  .status-sampled {
    margin-left: auto;
  }
  .status-sampled strong {
    color: #aab4c8;
    font-weight: 500;
  }
  .metrics {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 0.8rem;
    margin-bottom: 1rem;
  }
  .metric-card {
    border: 1px solid var(--line);
    border-radius: 9px;
    background: #121721;
    padding: 0.95rem 1rem 0.85rem;
    display: grid;
    gap: 0.3rem;
  }
  .metric-card > span {
    color: #9aa7bd;
    font-size: 0.78rem;
  }
  .metric-card > strong {
    font-size: 1.7rem;
    line-height: 1.25;
    font-weight: 600;
    letter-spacing: -0.04em;
  }
  .metric-card small {
    color: #8f9bb0;
    font-size: 0.75rem;
  }
  .attention-panel {
    border-color: #3a3020;
    background: linear-gradient(180deg, #181711 0%, #121721 6rem);
    margin-bottom: 1rem;
  }
  .attention-list {
    display: flex;
    flex-direction: column;
  }
  .attention-row {
    display: flex;
    align-items: center;
    gap: 0.7rem;
    padding: 0.6rem 0;
    border-top: 1px solid #2d2a22;
    flex-wrap: wrap;
  }
  .attention-row:first-child {
    border-top: 0;
  }
  .attention-row strong {
    font-weight: 600;
  }
  .attention-row > .muted {
    min-width: 8rem;
  }
  .attention-when {
    margin-left: auto;
    color: #c2a86e;
    font-size: 0.78rem;
  }
  .attention-row :global(button) {
    padding: 0.3rem 0.65rem;
    font-size: 0.78rem;
  }
  .overview-grid {
    display: grid;
    grid-template-columns: minmax(0, 1.65fr) minmax(19rem, 1fr);
    gap: 1rem;
    align-items: start;
  }
  .overview-worker {
    align-items: start;
  }
  .data-table {
    margin-top: 0.7rem;
  }
  .data-head,
  .data-row {
    display: grid;
    grid-template-columns: minmax(9rem, 1.5fr) 5.5rem minmax(6rem, 1fr) 6rem;
    gap: 0.6rem;
    align-items: center;
    padding: 0.55rem 0;
  }
  .data-head {
    color: #828fa5;
    font-size: 0.72rem;
    border-bottom: 1px solid var(--line);
    padding-top: 0;
  }
  .data-row {
    border-bottom: 1px solid #1e2531;
  }
  .data-row:last-child {
    border-bottom: 0;
  }
  .data-row strong {
    display: block;
    font-size: 0.86rem;
    font-weight: 600;
  }
  .row-sub {
    display: block;
    color: #7e8ba0;
    font-size: 0.7rem;
    margin-top: 0.15rem;
  }
  .identity-panel {
    position: sticky;
    top: 1rem;
  }
  .identity-list {
    margin: 0.4rem 0 0;
    grid-template-columns: 6.5rem minmax(0, 1fr);
    gap: 0.65rem 0.7rem;
    align-items: baseline;
  }
  .identity-list dt {
    color: #8c99ae;
    font-size: 0.74rem;
  }
  .identity-list dd {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    min-width: 0;
    margin: 0;
  }
  .identity-list dd > .mono {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .identity-list :global(.icon-button) {
    flex-shrink: 0;
    width: 1.55rem;
    height: 1.55rem;
    padding: 0;
  }
  .identity-list dd.identity-stack {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 0.15rem;
  }
  .identity-stack > span:first-child {
    flex-shrink: 0;
  }
  .identity-grants {
    margin-top: 0.7rem;
    border-top: 1px solid var(--line);
    padding-top: 0.6rem;
  }
  .identity-grants summary {
    color: #aeb9cc;
    cursor: pointer;
    font-size: 0.78rem;
  }
  .binding-detail {
    display: block;
    margin-top: 0.15rem;
  }
  .sync-entry {
    display: block;
  }
  .sync-peer {
    display: flex;
    justify-content: space-between;
    gap: 0.8rem;
    padding: 0.55rem 0;
    border-top: 1px solid var(--line);
    flex-wrap: wrap;
  }
  @media (max-width: 1050px) {
    .metrics {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
    .overview-grid {
      grid-template-columns: 1fr;
    }
    .identity-panel {
      position: static;
    }
  }
  @media (max-width: 700px) {
    .metrics {
      grid-template-columns: 1fr;
    }
    .status-sampled {
      margin-left: 0;
    }
    .data-head,
    .data-row {
      grid-template-columns: minmax(8rem, 1.4fr) 5rem 1fr;
    }
    .data-head > :nth-child(4),
    .data-row > :nth-child(4),
    .data-head > :nth-child(5),
    .data-row > :nth-child(5) {
      display: none;
    }
  }
</style>
