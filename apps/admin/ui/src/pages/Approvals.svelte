<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorMessage } from '../lib/api.svelte';
  import type {
    ApprovalStatus,
    ConnectionList,
    ConnectionRequest,
    ConnectionStatus,
    InterpreterProvider,
    ProviderManifest,
  } from '../lib/types';
  import Confirm from '../lib/Confirm.svelte';
  import Grants from '../lib/Grants.svelte';
  import Icon from '../lib/Icon.svelte';
  import ProviderList from '../lib/ProviderList.svelte';
  import { formatDateTime, microsDate, relativeTime, remainingTime, shortId } from '../lib/format';

  let {
    approval = null,
    oncount,
  }: {
    approval?: ApprovalStatus | null;
    oncount?: (count: number) => void;
  } = $props();

  let connections = $state<ConnectionRequest[]>([]);
  let providers = $state<InterpreterProvider[]>([]);
  let busy = $state(false);
  let error = $state('');
  let message = $state('');
  let now = $state(Date.now());
  let decision = $state<{ request: ConnectionRequest; allow: boolean } | null>(null);
  let replace = $state(false);
  let providerDecision = $state<InterpreterProvider | null>(null);
  let providerReplace = $state(false);

  const approvalBlocked = $derived(
    approval?.mode === 'deny' ||
      (approval?.terminal_required === true && approval.terminal_available === false),
  );
  const pendingConnections = $derived(
    connections.filter((request) => request.status === 'pending' && !isExpired(request)),
  );
  const pendingProviders = $derived(providers.filter((provider) => provider.status === 'pending'));
  const pendingCount = $derived(pendingConnections.length + pendingProviders.length);
  const history = $derived(
    connections
      .filter((request) => request.status !== 'pending' || isExpired(request))
      .map((request) => {
        const status = isExpired(request) ? 'expired' : request.status;
        return {
          key: request.connection_id,
          name: request.name,
          kind: '接続',
          result: historyResult(status),
          tone: historyTone(status),
          when: relativeTime(
            request.completed_at ?? request.approved_at ?? request.created_at,
            now,
          ),
        };
      }),
  );
  const decisionProviders = $derived(decision ? providersOf(decision.request) : []);

  function isExpired(request: ConnectionRequest): boolean {
    if (request.status !== 'pending') return false;
    const expires = microsDate(request.expires_at);
    return expires !== null && expires.getTime() <= now;
  }
  function providersOf(request: ConnectionRequest): ProviderManifest[] {
    if (request.providers?.length) return request.providers;
    return request.provider ? [request.provider] : [];
  }
  function historyResult(status: ConnectionStatus): string {
    switch (status) {
      case 'approved':
        return '承認';
      case 'completed':
        return '接続';
      case 'denied':
        return '拒否';
      case 'expired':
        return '期限切れ';
      default:
        return status;
    }
  }
  function historyTone(status: ConnectionStatus): string {
    switch (status) {
      case 'approved':
      case 'completed':
        return 'badge-ok';
      case 'denied':
      case 'expired':
        return 'badge-muted';
      default:
        return '';
    }
  }
  function permissionSummary(request: ConnectionRequest): string {
    const parts: string[] = [];
    if (request.grants.read) parts.push('読み取り');
    if (request.grants.write) parts.push('書き込み');
    if (request.grants.call) parts.push('Interpreter');
    if (request.grants.provide) parts.push('Provider');
    return parts.length ? parts.join(' / ') : 'なし';
  }
  async function load() {
    busy = true;
    error = '';
    try {
      const [connectionList, providerList] = await Promise.all([
        api<ConnectionList>('/connect-list', {}),
        api<{ providers: InterpreterProvider[] }>('/interpreter-list', {}),
      ]);
      connections = connectionList.connections;
      providers = providerList.providers;
      oncount?.(
        connections.filter((request) => request.status === 'pending' && !isExpired(request))
          .length + providers.filter((provider) => provider.status === 'pending').length,
      );
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  async function decide() {
    if (!decision) return;
    busy = true;
    error = '';
    try {
      await api('/connect-approve', {
        connection_id: decision.request.connection_id,
        allow: decision.allow,
        replace: decision.allow ? replace : false,
      });
      message = decision.allow ? '承認しました。アプリの完了通知待ちです。' : '拒否しました。';
      decision = null;
      replace = false;
      await load();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  async function approveProvider() {
    if (!providerDecision) return;
    busy = true;
    error = '';
    try {
      await api('/interpreter-approve', {
        provider: providerDecision.provider_id,
        replace: providerReplace,
      });
      message = `「${providerDecision.name}」を承認しました。`;
      providerDecision = null;
      providerReplace = false;
      await load();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  function openDecision(request: ConnectionRequest, allow: boolean) {
    decision = { request, allow };
    replace = false;
  }
  function openProvider(provider: InterpreterProvider) {
    providerDecision = provider;
    providerReplace = false;
  }
  onMount(() => {
    void load();
    const tick = setInterval(() => (now = Date.now()), 1000);
    const poll = setInterval(() => {
      if (!busy && !decision && !providerDecision && document.visibilityState === 'visible') {
        void load();
      }
    }, 10000);
    return () => {
      clearInterval(tick);
      clearInterval(poll);
    };
  });
</script>

<div class="page-title">
  <div>
    <h1>承認</h1>
  </div>
  <button class="icon-button" aria-label="更新" title="更新" disabled={busy} onclick={() => load()}>
    <Icon name="refresh" size={17} />
  </button>
</div>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if message}<p class="success" role="status">{message}</p>{/if}
{#if approvalBlocked}
  <div class="callout danger">この起動方法では承認できません。</div>
{/if}

{#if pendingCount === 0}
  <section class="panel">
    <p class="empty">承認待ちはありません。</p>
  </section>
{:else}
  <div class="request-list">
    {#each pendingConnections as request (request.connection_id)}
      {@const providerCount = providersOf(request).length}
      <article class="request-card">
        <div class="request-head">
          <span class="badge badge-info">接続</span>
          <strong>{request.name}</strong>
          <span class="muted small">{relativeTime(request.created_at, now)}</span>
          <span class="request-when">
            <span class="badge badge-warn">{remainingTime(request.expires_at, now)}</span>
          </span>
        </div>
        <div class="request-fields">
          <div>
            <span class="meta-label">Callback</span>
            <span class="mono request-value">{request.redirect_uri}</span>
          </div>
          <div>
            <span class="meta-label">権限</span>
            <span class="request-value">{permissionSummary(request)}</span>
          </div>
          <div>
            <span class="meta-label">Provider</span>
            <span class="request-value">{providerCount} 件</span>
          </div>
          <div>
            <span class="meta-label">Web origin</span>
            <span class="mono request-value">{request.web_origin ?? 'CLI / ネイティブアプリ'}</span>
          </div>
        </div>
        <div class="request-warning">
          <Icon name="alert" size={15} />
          <span>承認すると、アプリにトークンと Provider を発行します。</span>
        </div>
        <div class="request-actions">
          <button
            class="quiet danger-text"
            disabled={busy}
            onclick={() => openDecision(request, false)}
          >
            拒否
          </button>
          <button
            class="primary"
            disabled={busy || approvalBlocked}
            onclick={() => openDecision(request, true)}
          >
            承認
          </button>
        </div>
      </article>
    {/each}
    {#each pendingProviders as provider (provider.provider_id)}
      <article class="request-card">
        <div class="request-head">
          <span class="badge badge-ok">Provider</span>
          <strong>{provider.name}</strong>
          <span class="muted small">{provider.connected ? '接続中' : '接続前'}</span>
          <span class="request-when"><span class="badge badge-warn">承認待ち</span></span>
        </div>
        <div class="request-fields">
          <div>
            <span class="meta-label">Provider ID</span>
            <span class="mono request-value">{shortId(provider.provider_id, 12, 8)}</span>
          </div>
          <div>
            <span class="meta-label">契約</span>
            <span class="request-value">v{provider.contract_version}</span>
          </div>
          <div>
            <span class="meta-label">接続</span>
            <span class="request-value">{provider.connected ? '接続中' : '接続前'}</span>
          </div>
        </div>
        <div class="request-warning">
          <Icon name="alert" size={15} />
          <span>承認すると、このノードで Provider 名を使用できます。</span>
        </div>
        <div class="request-actions">
          <button
            class="primary"
            disabled={busy || approvalBlocked}
            onclick={() => openProvider(provider)}
          >
            承認
          </button>
        </div>
      </article>
    {/each}
  </div>
{/if}

{#if history.length}
  <section class="panel processed-panel">
    <div class="section-head">
      <div>
        <h2>処理済み</h2>
      </div>
      <span class="muted small">{history.length} 件</span>
    </div>
    <div class="data-table processed-table" role="table" aria-label="処理済みの承認">
      <div class="data-head" role="row">
        <span>名前</span><span>種類</span><span>結果</span><span>時刻</span>
      </div>
      {#each history as row (row.key)}
        <div class="data-row" role="row">
          <strong>{row.name}</strong>
          <span class="muted small">{row.kind}</span>
          <span class="badge {row.tone}">{row.result}</span>
          <span class="muted small">{row.when}</span>
        </div>
      {/each}
    </div>
  </section>
{/if}

{#if decision}
  <Confirm
    title={decision.allow ? '接続要求を承認' : '接続要求を拒否'}
    {busy}
    disabled={approvalBlocked}
    confirmLabel={decision.allow ? '承認する' : '拒否する'}
    danger={!decision.allow}
    onsubmit={decide}
    oncancel={() => {
      decision = null;
      replace = false;
    }}
  >
    <p><strong>{decision.request.name}</strong></p>
    {#if approval && decision.allow && approval.mode === 'terminal' && approval.terminal_available}
      <p class="muted small">承認すると、起動したターミナルで <kbd>y</kbd> を押します。</p>
    {:else if approval && decision.allow && approval.mode === 'session'}
      <p class="muted small">この起動方法では端末確認を行いません。</p>
    {/if}
    <dl class="confirm-grid">
      <dt>Callback</dt>
      <dd class="mono">{decision.request.redirect_uri}</dd>
      <dt>期限</dt>
      <dd>{formatDateTime(decision.request.expires_at)}</dd>
    </dl>
    {#if decision.allow}
      <h4>確定するアプリ権限</h4>
      <Grants grants={decision.request.grants} dense />
      <h4>登録する Provider（{decisionProviders.length}）</h4>
      <ProviderList
        providers={decisionProviders}
        replaceRequested={decision.request.provider_replace}
        showSchemas={false}
      />
      {#if decisionProviders.length}
        <label class="check">
          <input type="checkbox" bind:checked={replace} />
          同名の承認済み Provider を置換する
        </label>
      {/if}
    {:else}
      <p class="muted small">トークンと Provider は発行されません。</p>
    {/if}
  </Confirm>
{/if}

{#if providerDecision}
  <Confirm
    title="Provider を承認"
    {busy}
    disabled={approvalBlocked}
    confirmLabel="承認する"
    onsubmit={approveProvider}
    oncancel={() => {
      providerDecision = null;
      providerReplace = false;
    }}
  >
    <dl class="confirm-grid">
      <dt>名前</dt>
      <dd>{providerDecision.name}</dd>
      <dt>Provider ID</dt>
      <dd class="mono">{providerDecision.provider_id}</dd>
      <dt>契約</dt>
      <dd>v{providerDecision.contract_version}</dd>
    </dl>
    <label class="check">
      <input type="checkbox" bind:checked={providerReplace} />
      同じ名前の承認済み提供元を置換する
    </label>
    {#if approval && approval.mode === 'terminal' && approval.terminal_available}
      <p class="muted small">承認すると、起動したターミナルで <kbd>y</kbd> を押します。</p>
    {:else if approval && approval.mode === 'session'}
      <p class="muted small">この起動方法では端末確認を行いません。</p>
    {/if}
  </Confirm>
{/if}

<style>
  .request-list {
    display: flex;
    flex-direction: column;
    gap: 0.85rem;
  }
  .request-card {
    border: 1px solid #3a3020;
    border-radius: 10px;
    background: linear-gradient(180deg, #171611 0%, #121721 8rem);
    padding: 1rem 1.1rem 0.85rem;
  }
  .request-head {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    flex-wrap: wrap;
  }
  .request-head strong {
    font-size: 1rem;
    font-weight: 600;
  }
  .request-head .muted {
    font-size: 0.75rem;
  }
  .request-when {
    margin-left: auto;
  }
  .request-fields {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 0.8rem 1rem;
    margin-top: 0.9rem;
  }
  .request-value {
    display: block;
    margin-top: 0.15rem;
    font-size: 0.8rem;
    overflow-wrap: anywhere;
  }
  .request-warning {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    border: 1px solid #3a3020;
    background: #1c1911;
    border-radius: 7px;
    padding: 0.45rem 0.6rem;
    margin-top: 0.85rem;
    color: #d7bd80;
    font-size: 0.78rem;
  }
  .request-actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-top: 0.55rem;
  }
  .danger-text {
    color: #ffb1ac;
  }
  .processed-panel {
    margin-top: 1.2rem;
  }
  .processed-table .data-head,
  .processed-table .data-row {
    grid-template-columns: minmax(9rem, 1.5fr) 6rem 7rem 8rem;
  }
  .processed-table .data-row {
    padding: 0.55rem 0;
  }
  .processed-table .data-row strong {
    font-size: 0.84rem;
  }
  @media (max-width: 850px) {
    .request-fields {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
  @media (max-width: 600px) {
    .request-fields {
      grid-template-columns: 1fr;
    }
    .request-when {
      margin-left: 0;
    }
    .processed-table .data-head,
    .processed-table .data-row {
      grid-template-columns: minmax(8rem, 1.5fr) 5rem 6rem;
    }
    .processed-table .data-head > :last-child,
    .processed-table .data-row > :last-child {
      display: none;
    }
  }
</style>
