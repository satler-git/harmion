<script lang="ts">
  import type { Capabilities, Overview, Page } from '../lib/types';
  import { count, peersOf, short, workerAdmin } from '../lib/api.svelte';
  import Clipboard from '../lib/Clipboard.svelte';
  import Grants from '../lib/Grants.svelte';
  import WorkerRegistration from '../lib/WorkerRegistration.svelte';
  import {
    formatDateTime,
    membershipStateLabel,
    membershipStateTone,
    relativeTime,
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
  const pinnedPeers = $derived(peers.filter((peer) => peer.pinned).length);
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
  const configIssues = $derived([
    ...new Set([...(hostStatus?.config.missing ?? []), ...(hostStatus?.config.invalid ?? [])]),
  ]);
  const status = $derived.by((): { tone: string; label: string } | null => {
    if (!worker) return null;
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

  // A refresh can reveal a changed identity/origin while the stored identity is
  // still the old one; remount so the registration code is fetched from the
  // current deployment rather than reused.
  const registrationKey = $derived(
    [
      sampledAt,
      overview?.identity.node_id,
      overview?.identity.signing_key,
      hostStatus?.provisioning.identity_matches,
      bindingDetail,
    ].join('|'),
  );
</script>

<div class="page-title">
  <div>
    <h1>概要</h1>
  </div>
  <button disabled={busy} onclick={refresh}>{busy ? '取得中…' : '更新'}</button>
</div>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if overview}
  {#if worker && status}
    <div class="status-strip">
      <span class="badge {status.tone}">{status.label}</span>
      {#if hostStatus?.provisioning.bootstrap_enabled}
        <span class="badge badge-warn">初期設定 受付中</span>
      {/if}
      {#if hostStatus?.provisioning.allow_reprovision}
        <span class="badge badge-warn">再初期化 許可</span>
      {/if}
      {#if configIssues.length}
        <span class="badge badge-danger">設定 {configIssues.length} 件</span>
      {/if}
    </div>
  {/if}
  {#if worker && workerAdmin.statusError && !hostStatus}
    <p class="warning" role="status">
      {workerAdmin.statusCode === 404
        ? 'この Worker は状態取得に対応していません。'
        : '状態を取得できませんでした。'}
    </p>
  {/if}

  {#if worker}
    <div class="stats">
      <article>
        <span>最終受信</span><strong>{lastInboundText}</strong>
        <small>{lastInboundDetail}</small>
      </article>
      <article>
        <span>保持する更新</span><strong>{stats ? count(stats.accepted_updates) : '—'}</strong>
        <small>{stats ? bytes(stats.record_bytes) + ' の履歴' : '集計できません'}</small>
      </article>
      <article>
        <span>Content</span><strong>{stats ? count(stats.contents) : '—'}</strong>
        <small>削除済みを含む</small>
      </article>
      <article>
        <span>ノード</span><strong>{peers.length}</strong>
        <small>pin {pinnedPeers} · 参加 {activeMembers}</small>
      </article>
    </div>
  {:else if overview.metrics}
    <div class="stats">
      <article>
        <span>Content</span><strong>{count(overview.metrics.visible_contents)}</strong>
        <small>削除込み {count(overview.metrics.contents)}</small>
      </article>
      <article>
        <span>受信した更新</span><strong>{count(overview.metrics.accepted_updates)}</strong>
      </article>
      <article>
        <span>データ量</span><strong>{bytes(overview.metrics.record_bytes)}</strong>
        <small>更新レコード</small>
      </article>
      <article>
        <span>アプリ</span><strong>{count(overview.metrics.active_apps)}</strong>
      </article>
    </div>
  {/if}

  <div class="two-columns">
    <section class="panel">
      <div class="section-head">
        <div>
          <h2>識別情報</h2>
        </div>
      </div>
      <dl>
        <dt>Node ID</dt>
        <dd class="mono id-line">
          <span>{overview.identity.node_id}</span>
          <Clipboard text={overview.identity.node_id} label="コピー" />
        </dd>
        <dt>Cluster ID</dt>
        <dd class="mono id-line">
          <span>{overview.identity.cluster_id}</span>
          <Clipboard text={overview.identity.cluster_id} label="コピー" />
        </dd>
        {#if worker && overview.identity.epoch}
          <dt>Epoch</dt>
          <dd class="mono id-line">
            <span class="small">{overview.identity.epoch}</span>
            <Clipboard text={overview.identity.epoch} label="コピー" />
          </dd>
        {/if}
        <dt>接続方式</dt>
        <dd>
          {binding ? transportLabel(binding.transport) : '—'}
          {#if bindingDetail}<span class="mono muted small binding-detail">
              {bindingDetail}
            </span>{/if}
        </dd>
        {#if overview.identity.signing_key}
          <dt>署名公開鍵</dt>
          <dd class="mono small">{overview.identity.signing_key}</dd>
        {/if}
        {#if worker && overview.identity.cluster_root}
          <dt>Cluster Root</dt>
          <dd>
            <details>
              <summary class="mono small">{short(overview.identity.cluster_root)}</summary>
              <p class="mono small">{overview.identity.cluster_root}</p>
            </details>
          </dd>
        {/if}
        {#if worker}
          <dt>自分の参加状態</dt>
          <dd><span class="badge {selfTone}">{selfLabel}</span></dd>
          <dt>初期設定</dt>
          <dd>
            {overview.configured ? '済み' : '未設定'}
            {#if hostStatus?.provisioning.provisioned_at}
              <span class="muted small">
                · {formatDateTime(hostStatus.provisioning.provisioned_at)}
              </span>
            {/if}
            {#if hostStatus?.provisioning.provisioned}
              <span
                class="badge {hostStatus.provisioning.identity_matches
                  ? 'badge-ok'
                  : 'badge-danger'}"
              >
                {hostStatus.provisioning.identity_matches ? '設定と一致' : '設定と不一致'}
              </span>
            {/if}
          </dd>
        {/if}
        {#if overview.permissions}
          <dt>権限</dt>
          <dd><Grants grants={overview.permissions} dense /></dd>
        {/if}
      </dl>
    </section>

    {#if worker}
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
          <dd>
            {syncIntervalText}
          </dd>
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
    {:else}
      <div class="stack">
        <section class="panel">
          <div class="section-head">
            <div>
              <h2>接続</h2>
            </div>
          </div>
          <dl>
            <dt>参加が有効なノード</dt>
            <dd>{activeMembers} 台</dd>
            <dt>保留中の退役証明</dt>
            <dd>{overview.members.pending_retirements.length} 件</dd>
            {#if approvalCount > 0}
              <dt>承認待ち</dt>
              <dd>{approvalCount} 件</dd>
            {/if}
            <dt>OAuth</dt>
            <dd>
              {overview.capabilities.oauth ? '有効' : '無効'}
              {#if oauthOrigin}<span class="mono muted small binding-detail">
                  {oauthOrigin}
                </span>{/if}
            </dd>
          </dl>
          <div class="actions">
            <button onclick={() => navigate('peers')}>ノード</button>
            {#if !readOnly}<button onclick={() => navigate('approvals')}>承認</button>{/if}
            {#if !readOnly}<button class="primary" onclick={() => navigate('apps')}>
                アプリ
              </button>{/if}
          </div>
        </section>
        <section class="panel">
          <div class="section-head">
            <div>
              <h2>ノード</h2>
            </div>
            <button class="quiet" onclick={() => navigate('peers')}>すべて</button>
          </div>
          {#if overview.members.members.length === 0}
            <p class="empty">参加ノードはありません。</p>
          {:else}
            {#each overview.members.members as member}
              <div class="node-row">
                <div class="title-line">
                  <strong>
                    {member.node_id === overview.identity.node_id
                      ? 'このノード'
                      : short(member.node_id)}
                  </strong>
                  <span class="badge {membershipStateTone(member.state)}">
                    {membershipStateLabel(member.state)}
                  </span>
                </div>
                <p class="mono muted small" title={member.node_id}>{short(member.node_id)}</p>
              </div>
            {/each}
          {/if}
        </section>
      </div>
    {/if}
  </div>

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

  <p class="muted small">取得 {sampledAt || '—'}</p>
{:else if busy}
  <p class="empty">ノードの状態を取得しています…</p>
{/if}

<style>
  .id-line {
    display: flex;
    align-items: start;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .id-line > span {
    min-width: 0;
  }
  .id-line :global(button) {
    padding: 0.15rem 0.45rem;
    font-size: 0.72rem;
    flex-shrink: 0;
  }
  .binding-detail {
    display: block;
    margin-top: 0.2rem;
  }
  .panel summary {
    color: var(--accent);
    cursor: pointer;
    font-size: 0.8rem;
  }
  .panel summary + p {
    margin: 0.4rem 0 0;
  }
  .section-head {
    margin-bottom: 0.4rem;
  }
  .section-head + dl {
    margin-top: 0.8rem;
  }
  .panel dl :global(.grant-list) {
    margin: 0;
  }
  .status-strip {
    display: inline-flex;
    align-items: center;
    gap: 0.7rem;
    flex-wrap: wrap;
    border: 1px solid #2a3448;
    border-radius: 10px;
    background: #0e131c;
    padding: 0.55rem 0.8rem;
    margin: 1.2rem 0 0;
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
  .stack {
    display: flex;
    flex-direction: column;
    gap: 1.3rem;
  }
  .node-row {
    padding: 0.6rem 0;
    border-top: 1px solid var(--line);
  }
  .node-row:first-of-type {
    border-top: 0;
  }
  .node-row:first-of-type {
    padding-top: 0.2rem;
  }
  .node-row p {
    margin: 0.2rem 0 0;
  }
  .title-line {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .title-line strong {
    font-size: 0.9rem;
  }
  .issues {
    margin: 0.6rem 0 0;
    padding-left: 1.2rem;
    color: #e5c28d;
  }
</style>
