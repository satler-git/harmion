<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorMessage, short } from '../lib/api.svelte';
  import type {
    ApprovalStatus,
    ConnectionList,
    ConnectionRequest,
    ConnectionStatus,
    ProviderManifest,
  } from '../lib/types';
  import Confirm from '../lib/Confirm.svelte';
  import Grants from '../lib/Grants.svelte';
  import ProviderList from '../lib/ProviderList.svelte';
  import {
    approvalModeLabel,
    approvalTone,
    connectionStatusLabel,
    connectionStatusTone,
    formatDateTime,
    microsDate,
    prettyJson,
    providerHint,
    relativeTime,
    remainingTime,
    shortId,
  } from '../lib/format';

  let { approval = null }: { approval?: ApprovalStatus | null } = $props();

  let connections = $state<ConnectionRequest[]>([]);
  let busy = $state(false),
    polling = $state(false),
    error = $state(''),
    message = $state('');
  let decision = $state<{ request: ConnectionRequest; allow: boolean } | null>(null),
    replace = $state(false);
  let tab = $state<'all' | 'pending' | 'history'>('all');
  let now = $state(Date.now());

  const pending = $derived(
    connections.filter((request) => request.status === 'pending' && !isExpired(request)),
  );
  const history = $derived(
    connections.filter((request) => request.status !== 'pending' || isExpired(request)),
  );
  const visible = $derived(tab === 'pending' ? pending : tab === 'history' ? history : connections);
  const previewProviders = $derived(
    decision ? providersOf(decision.request) : ([] as ProviderManifest[]),
  );
  const approvalBlocked = $derived(
    approval?.mode === 'deny' ||
      (approval?.terminal_required === true && approval.terminal_available === false),
  );

  function providersOf(request: ConnectionRequest): ProviderManifest[] {
    if (request.providers?.length) return request.providers;
    return request.provider ? [request.provider] : [];
  }
  function isExpired(request: ConnectionRequest): boolean {
    if (request.status !== 'pending') return false;
    const expires = microsDate(request.expires_at);
    return expires !== null && expires.getTime() <= now;
  }
  function statusDetail(status: ConnectionStatus, request: ConnectionRequest): string {
    switch (status) {
      case 'pending':
        return `${remainingTime(request.expires_at, now)}。承認するとアプリの権限と Provider 登録が確定します。`;
      case 'approved':
        return '所有者の承認は完了しています。アプリの完了通知（connect-complete）待ちです。';
      case 'completed':
        return `接続完了 ${formatDateTime(request.completed_at)}。アプリはトークンを取得済みです。`;
      case 'denied':
        return '所有者が拒否しました。トークンも Provider も発行されません。';
      case 'expired':
        return `承認されないまま期限が切れました（${formatDateTime(request.expires_at)}）。新しい要求を作り直してください。`;
    }
  }
  async function load(silent = false) {
    if (silent) polling = true;
    else busy = true;
    if (!silent) error = '';
    try {
      connections = (await api<ConnectionList>('/connect-list', {})).connections;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      if (silent) polling = false;
      else busy = false;
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
      message = decision.allow
        ? '接続を承認しました。アプリの完了通知が届くと接続完了になります。'
        : '接続要求を拒否しました。トークンも Provider も発行されていません。';
      decision = null;
      replace = false;
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
  onMount(() => {
    void load();
    const tick = setInterval(() => {
      now = Date.now();
    }, 1000);
    const poll = setInterval(() => {
      if (!busy && !polling && !decision && document.visibilityState === 'visible') void load(true);
    }, 5000);
    return () => {
      clearInterval(tick);
      clearInterval(poll);
    };
  });
</script>

<div class="page-title">
  <div>
    <p class="eyebrow">INTEGRATED CONNECTION</p>
    <h1>接続要求</h1>
    <p class="muted">
      アプリの初回接続を、アプリ権限の確定・OAuth クライアント作成・Provider
      登録の1回の承認にまとめます。 承認後にアプリがトークンを受け取ると「接続完了」になります。
    </p>
  </div>
  <button class="quiet" disabled={busy} onclick={() => load()}>
    {busy ? '更新中…' : '再読み込み'}
  </button>
</div>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if message}<p class="success" role="status">{message}</p>{/if}
{#if pending.length}
  <div class="callout warn">
    <strong>承認待ちが {pending.length} 件あります。</strong>
    <span>内容を確認して承認または拒否してください。</span>
  </div>
{/if}
{#if pending.length && approval?.mode === 'terminal' && approval.terminal_available}
  <div class="callout">
    <strong>承認時の端末確認</strong>
    <span>
      承認を実行すると、管理アプリを起動したターミナルで <kbd>y</kbd> を押す必要があります。 ブラウザセッションの窃取だけでは承認を完結させないための確認です。
    </span>
  </div>
{/if}
{#if approvalBlocked}
  <div class="callout danger">
    <strong>この起動方法では R2 の承認を完了できません。</strong>
    <span>{approval ? approvalModeLabel(approval) : 'R2 の承認を完了できません。'}</span>
  </div>
{/if}

<div class="segmented" role="tablist" aria-label="表示する接続要求">
  <button
    role="tab"
    aria-selected={tab === 'all'}
    class:active={tab === 'all'}
    onclick={() => (tab = 'all')}
  >
    すべて <span class="count">{connections.length}</span>
  </button>
  <button
    role="tab"
    aria-selected={tab === 'pending'}
    class:active={tab === 'pending'}
    onclick={() => (tab = 'pending')}
  >
    承認待ち <span class="count">{pending.length}</span>
  </button>
  <button
    role="tab"
    aria-selected={tab === 'history'}
    class:active={tab === 'history'}
    onclick={() => (tab = 'history')}
  >
    履歴 <span class="count">{history.length}</span>
  </button>
</div>

<section class="panel">
  <div class="section-head">
    <div>
      <h2>
        {tab === 'pending' ? '承認待ちの要求' : tab === 'history' ? '最近の履歴' : '最近の接続要求'}
      </h2>
      <p class="muted small">
        最新 32
        件を表示します。表示中は5秒ごとに自動更新し、期限切れの要求は「期限切れ」へ移ります。
      </p>
    </div>
    <span class="muted small">{visible.length} 件</span>
  </div>
  {#if connections.length === 0}
    <p class="empty">接続要求はありません。アプリが接続を開始すると、ここに要求が表示されます。</p>
  {:else if visible.length === 0}
    <p class="empty">この条件に一致する接続要求はありません。</p>
  {:else}
    <div class="connection-list">
      {#each visible as request (request.connection_id)}
        {@const providers = providersOf(request)}
        {@const status = isExpired(request) ? 'expired' : request.status}
        <article class="connection-card" class:pending={status === 'pending'}>
          <div class="connection-head">
            <div class="connection-title">
              <div class="title-line">
                <h3>{request.name}</h3>
                <span class="badge {connectionStatusTone(status)}">
                  {connectionStatusLabel(status)}
                </span>
              </div>
              <p class="mono muted small">{request.connection_id}</p>
            </div>
            <div class="connection-when">
              <span>{relativeTime(request.created_at, now)}に作成</span>
              {#if status === 'pending'}
                <span class="badge badge-warn">{remainingTime(request.expires_at, now)}</span>
              {:else}
                <span class="muted small">{formatDateTime(request.expires_at)} まで</span>
              {/if}
            </div>
          </div>

          <p class:warning={status === 'pending'} class="status-detail">
            {statusDetail(status, request)}
          </p>

          <div class="connection-meta">
            <div>
              <span class="meta-label">Callback</span>
              <span class="mono">{request.redirect_uri}</span>
            </div>
            <div>
              <span class="meta-label">Web origin</span>
              <span class="mono">{request.web_origin ?? '指定なし（CLI / ネイティブアプリ）'}</span>
            </div>
            <div>
              <span class="meta-label">OAuth クライアント</span>
              <span class="mono">
                {request.client_id ? shortId(request.client_id) : '承認時に新規作成'}
              </span>
            </div>
          </div>

          <div class="connection-columns">
            <section class="connection-section">
              <h4>要求された権限</h4>
              <Grants grants={request.grants} />
            </section>
            <section class="connection-section">
              <h4>
                登録する Provider <span class="count">{providers.length}</span>
              </h4>
              {#if request.provider_replace}
                <p class="warning small">
                  アプリは同名 Provider の置換を希望しています。承認時に「置換する」を選ばないと、
                  同名の承認済み Provider がある場合は承認に失敗します。
                </p>
              {/if}
              <ProviderList {providers} replaceRequested={request.provider_replace} />
              <p class="muted small provider-hint">{providerHint(request.grants)}</p>
            </section>
          </div>

          <details class="technical">
            <summary>ID・時刻・確定後の情報を表示</summary>
            <dl class="technical-grid">
              <dt>Connection ID</dt>
              <dd class="mono">{request.connection_id}</dd>
              <dt>作成</dt>
              <dd>{formatDateTime(request.created_at)}</dd>
              <dt>期限</dt>
              <dd>{formatDateTime(request.expires_at)}</dd>
              {#if request.approved_at}<dt>承認</dt>
                <dd>{formatDateTime(request.approved_at)}</dd>{/if}
              {#if request.completed_at}<dt>完了</dt>
                <dd>{formatDateTime(request.completed_at)}</dd>{/if}
              {#if request.client_id}<dt>Client ID</dt>
                <dd class="mono">{request.client_id}</dd>{/if}
              {#if request.app_id}<dt>App ID</dt>
                <dd class="mono">{request.app_id}</dd>{/if}
              {#if request.provider_ids?.length}
                <dt>Provider ID</dt>
                <dd class="mono">{request.provider_ids.join('\n')}</dd>
              {:else if request.provider_id}
                <dt>Provider ID</dt>
                <dd class="mono">{request.provider_id}</dd>
              {/if}
              <dt>Grant JSON</dt>
              <dd><pre class="json-text">{prettyJson(request.grants)}</pre></dd>
            </dl>
          </details>

          {#if status === 'pending'}
            <div class="actions">
              <button class="primary" disabled={busy} onclick={() => openDecision(request, true)}>
                承認内容を確認
              </button>
              <button class="danger" disabled={busy} onclick={() => openDecision(request, false)}>
                拒否
              </button>
            </div>
          {:else if status === 'approved'}
            <p class="muted small">
              この要求は承認済みです。アプリが完了通知を送るまで、ここで状態を確認できます。
            </p>
          {:else if status === 'completed'}
            <p class="muted small">
              {request.provider_ids?.length ?? providers.length} 件の Provider とアプリのトークンが発行されています。
            </p>
          {/if}
        </article>
      {/each}
    </div>
  {/if}
</section>

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
    <p><strong>{decision.request.name}</strong> からの接続要求です。</p>
    {#if approval}
      <div class="approval-line">
        <span class="badge {approvalTone(approval)}">端末承認</span>
        <span class="small">{approvalModeLabel(approval)}</span>
      </div>
      {#if decision.allow && approval.mode === 'terminal' && approval.terminal_available}
        <p class="muted small">
          下の「承認する」を押した後、管理アプリを起動したターミナルで <kbd>y</kbd> を押すと 承認が確定します。
        </p>
      {:else if decision.allow && approval.mode === 'session'}
        <p class="muted small">
          この起動方法では端末確認を行いません。ブラウザのこの確認だけが最終確認になります。
        </p>
      {/if}
    {/if}
    <dl class="confirm-grid">
      <dt>Callback</dt>
      <dd class="mono">{decision.request.redirect_uri}</dd>
      <dt>Web origin</dt>
      <dd class="mono">{decision.request.web_origin ?? '指定なし'}</dd>
      <dt>OAuth クライアント</dt>
      <dd class="mono">
        {decision.request.client_id ? short(decision.request.client_id) : '承認時に新規作成'}
      </dd>
    </dl>
    {#if decision.allow}
      <h4>確定するアプリ権限</h4>
      <Grants grants={decision.request.grants} dense />
      <h4>登録する Provider（{previewProviders.length}）</h4>
      <ProviderList
        providers={previewProviders}
        replaceRequested={decision.request.provider_replace}
        showSchemas={false}
      />
      {#if previewProviders.length}
        {#if decision.request.provider_replace}
          <p class="muted small">
            アプリは同名 Provider の置換を希望しています。既存の同名 Provider を置き換える場合だけ、
            次を選んでください。
          </p>
        {:else}
          <p class="muted small">
            同名の承認済み Provider が既にある場合、置換を選ばないと承認は失敗します。
            心当たりがない場合はオフのままにしてください。
          </p>
        {/if}
        <label class="check">
          <input type="checkbox" bind:checked={replace} />
          同名の承認済み Provider を置換する
        </label>
      {/if}
      <p class="muted small">
        承認すると、アプリ grant の確定、OAuth クライアントの作成、Provider
        の登録・承認を1つの操作で行います。トークンはアプリが完了通知を送った時点で発行されます。
      </p>
    {:else}
      <p class="muted small">
        この要求には資格情報も Provider も発行されません。アプリ側には拒否として通知されます。
      </p>
    {/if}
  </Confirm>
{/if}

<style>
  .connection-list {
    display: grid;
    gap: 1rem;
  }
  .connection-card {
    border: 1px solid var(--line);
    border-radius: 10px;
    padding: 1rem 1.1rem;
    display: grid;
    gap: 0.8rem;
    background: #0f141e;
  }
  .connection-card.pending {
    border-color: #5a4630;
    background: linear-gradient(180deg, #181711 0%, #0f141e 9rem);
  }
  .connection-head {
    display: flex;
    align-items: start;
    justify-content: space-between;
    gap: 1rem;
  }
  .connection-title {
    min-width: 0;
  }
  .title-line {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    flex-wrap: wrap;
  }
  .connection-title h3 {
    margin: 0;
    font-size: 1.05rem;
  }
  .connection-title > p {
    margin: 0.25rem 0 0;
  }
  .connection-when {
    display: flex;
    flex-direction: column;
    align-items: end;
    gap: 0.35rem;
    color: var(--muted);
    font-size: 0.78rem;
    white-space: nowrap;
  }
  .status-detail {
    margin: 0;
    font-size: 0.85rem;
    color: #b8c2d6;
  }
  .connection-meta {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 16rem), 1fr));
    gap: 0.55rem 0.9rem;
    padding: 0.7rem 0.8rem;
    border: 1px solid #202939;
    border-radius: 8px;
    background: #0b1019;
  }
  .connection-meta > div {
    min-width: 0;
  }
  .connection-meta span {
    display: block;
  }
  .connection-columns {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1.15fr);
    gap: 1rem;
  }
  .connection-section h4 {
    margin: 0 0 0.5rem;
    font-size: 0.82rem;
    color: #b9c6e6;
    font-weight: 600;
  }
  .provider-hint {
    margin: 0.55rem 0 0;
  }
  .technical {
    border-top: 1px solid var(--line);
    padding-top: 0.55rem;
  }
  .technical summary {
    color: var(--accent);
    cursor: pointer;
    font-size: 0.8rem;
  }
  .technical-grid {
    display: grid;
    grid-template-columns: 8.5rem minmax(0, 1fr);
    gap: 0.35rem 0.9rem;
    margin: 0.7rem 0 0;
  }
  .technical-grid dt {
    color: var(--muted);
    font-size: 0.78rem;
  }
  .technical-grid dd {
    margin: 0;
    font-size: 0.82rem;
  }
  .technical-grid .json-text {
    margin: 0;
    max-height: 16rem;
    overflow: auto;
  }
  .confirm-grid {
    grid-template-columns: 7rem minmax(0, 1fr);
    gap: 0.35rem 0.9rem;
    margin: 0.8rem 0;
  }
  .confirm-grid dt {
    font-size: 0.8rem;
  }
  .confirm-grid dd {
    font-size: 0.84rem;
  }
  .confirm-grid + h4 {
    margin-top: 0.6rem;
  }
  @media (max-width: 1080px) {
    .connection-columns {
      grid-template-columns: 1fr;
    }
  }
  @media (max-width: 730px) {
    .connection-head {
      flex-direction: column;
    }
    .connection-when {
      align-items: start;
      flex-direction: row;
      flex-wrap: wrap;
    }
    .technical-grid,
    .confirm-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
