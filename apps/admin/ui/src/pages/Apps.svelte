<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorMessage } from '../lib/api.svelte';
  import type {
    AppPreview,
    ConnectionList,
    ConnectionRequest,
    Filter,
    InterpreterProvider,
    Overview,
    Page,
    ProviderManifest,
  } from '../lib/types';
  import Clipboard from '../lib/Clipboard.svelte';
  import Confirm from '../lib/Confirm.svelte';
  import Grants from '../lib/Grants.svelte';
  import Icon from '../lib/Icon.svelte';
  import { formatDateTime, shortId } from '../lib/format';

  let { overview, navigate }: { overview: Overview | null; navigate: (page: Page) => void } =
    $props();

  let input = $state(''),
    preview = $state<AppPreview | null>(null),
    busy = $state(false),
    error = $state(''),
    message = $state('');
  let registered = $state(''),
    consent = $state(''),
    appId = $state(''),
    readMode = $state('none'),
    writeMode = $state('none');
  let readKinds = $state(''),
    writeKinds = $state(''),
    callGrant = $state(false),
    provideGrant = $state(false),
    provideNames = $state(''),
    revoke = $state(false),
    changing = $state(false);
  let connections = $state<ConnectionRequest[]>([]);
  let providers = $state<InterpreterProvider[]>([]);
  let statusError = $state('');

  const oauthEnabled = $derived(overview?.capabilities.oauth ?? false);
  const connected = $derived(
    connections.filter(
      (request) => request.status === 'approved' || request.status === 'completed',
    ),
  );
  const approvedProviders = $derived(
    providers.filter((provider) => provider.status === 'approved'),
  );
  const history = $derived(
    connections.filter(
      (request) =>
        request.status === 'completed' ||
        request.status === 'denied' ||
        request.status === 'expired',
    ),
  );

  function providersOf(request: ConnectionRequest): ProviderManifest[] {
    if (request.providers?.length) return request.providers;
    return request.provider ? [request.provider] : [];
  }
  function permissionSummary(request: ConnectionRequest): string {
    const parts: string[] = [];
    if (request.grants.read) parts.push('読み取り');
    if (request.grants.write) parts.push('書き込み');
    if (request.grants.call) parts.push('Interpreter');
    if (request.grants.provide) parts.push('Provider');
    return parts.length ? parts.join(' / ') : 'なし';
  }
  function connectionState(request: ConnectionRequest): string {
    return request.status === 'completed' ? '接続中' : '承認済み';
  }
  async function loadStatus() {
    statusError = '';
    try {
      const [connectionList, providerList] = await Promise.all([
        api<ConnectionList>('/connect-list', {}),
        api<{ providers: InterpreterProvider[] }>('/interpreter-list', {}),
      ]);
      connections = connectionList.connections;
      providers = providerList.providers;
    } catch (e) {
      statusError = errorMessage(e);
    }
  }
  async function review() {
    busy = true;
    error = '';
    preview = null;
    consent = '';
    registered = '';
    const submitted = input;
    try {
      const result = await api<AppPreview>('/app-preview', { input: submitted });
      if (input === submitted) preview = result;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  async function approve() {
    if (!preview) return;
    busy = true;
    error = '';
    try {
      if (preview.type === 'registration') {
        const result = await api<{
          client_id: string;
          redirect_uri: string;
          authorization_endpoint: string;
          token_endpoint: string;
        }>('/oauth-register', { name: preview.name, redirect_uri: preview.redirect_uri });
        // This package contains public registration information for the requesting app.
        registered = JSON.stringify(result);
        message = 'アプリを登録しました。この1行を要求元アプリへ渡してください。';
      } else {
        const result = await api<{ url: string }>('/oauth-consent', { request: preview.request });
        const url = new URL(result.url);
        if (url.origin !== overview?.capabilities.oauth_origin || url.pathname !== '/oauth/consent')
          throw new Error('認可画面の接続先が一致しません。');
        consent = url.href;
        message = '認可画面で要求を確認し、許可または拒否してください。';
      }
      preview = null;
      void loadStatus();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  function filter(mode: string, kinds: string): Filter | null {
    return mode === 'none'
      ? null
      : mode === 'all'
        ? { type: 'all' }
        : {
            type: 'kinds',
            kinds: [
              ...new Set(
                kinds
                  .split(/[,\n]/)
                  .map((kind) => kind.trim())
                  .filter(Boolean),
              ),
            ],
          };
  }
  function names(): string[] {
    return [
      ...new Set(
        provideNames
          .split(/[,\n]/)
          .map((name) => name.trim())
          .filter(Boolean),
      ),
    ];
  }
  async function change() {
    busy = true;
    error = '';
    try {
      await api('/change-grants', {
        app_id: appId.trim(),
        revoked: revoke,
        grants: {
          read: filter(readMode, readKinds),
          write: filter(writeMode, writeKinds),
          call: callGrant,
          provide: provideGrant,
          provide_names: provideGrant ? names() : [],
          manage_apps: false,
          manage_nodes: false,
        },
      });
      changing = false;
      message = revoke
        ? 'アプリを失効させ、指定した権限を反映しました。'
        : 'アプリの権限を置き換えました。';
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  onMount(() => {
    void loadStatus();
  });
</script>

<div class="page-title">
  <div>
    <h1>アプリ</h1>
  </div>
  <div class="actions">
    <button class="primary" onclick={() => navigate('approvals')}>接続を許可</button>
    <button class="icon-button" aria-label="更新" title="更新" onclick={loadStatus}>
      <Icon name="refresh" size={17} />
    </button>
  </div>
</div>

{#if !oauthEnabled}
  <p class="warning">
    このノードの OAuth は無効です。ノードの OAuth 設定を有効にしてからアプリを接続してください。
  </p>
{/if}
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if statusError}<p class="warning" role="status">{statusError}</p>{/if}
{#if message}<p class="success" role="status">{message}</p>{/if}

<div class="app-columns">
  <section class="panel">
    <div class="section-head">
      <div>
        <h2>接続済み</h2>
      </div>
      <span class="muted small">{connected.length}</span>
    </div>
    {#if connected.length === 0}
      <p class="empty">接続済みのアプリはありません。</p>
    {:else}
      <div class="app-cards">
        {#each connected as request (request.connection_id)}
          <article class="app-card">
            <div class="app-card-head">
              <strong>{request.name}</strong>
              <span class="badge {request.status === 'completed' ? 'badge-ok' : 'badge-info'}">
                {connectionState(request)}
              </span>
            </div>
            <p class="muted small">{permissionSummary(request)}</p>
            <p class="mono muted small">{request.redirect_uri}</p>
            <p class="muted small">
              {request.completed_at
                ? `接続 ${formatDateTime(request.completed_at)}`
                : `承認 ${formatDateTime(request.approved_at)}`}
            </p>
          </article>
        {/each}
      </div>
    {/if}
  </section>

  <section class="panel">
    <div class="section-head">
      <div>
        <h2>Provider</h2>
      </div>
      <span class="muted small">{approvedProviders.length}</span>
    </div>
    {#if approvedProviders.length === 0}
      <p class="empty">承認済みの Provider はありません。</p>
    {:else}
      <div class="provider-stack">
        {#each approvedProviders as provider (provider.provider_id)}
          <article class="provider-row">
            <div>
              <strong>{provider.name}</strong>
              <p class="mono muted small">{shortId(provider.provider_id, 12, 8)}</p>
            </div>
            <div class="provider-meta">
              <span class="badge badge-ok">承認済み</span>
              <span class="muted small">v{provider.contract_version}</span>
              <span class="muted small">{provider.connected ? '接続中' : '未接続'}</span>
            </div>
          </article>
        {/each}
      </div>
    {/if}
  </section>
</div>

<section class="panel">
  <div class="section-head">
    <div>
      <h2>接続履歴</h2>
    </div>
    <span class="muted small">{history.length}</span>
  </div>
  {#if history.length === 0}
    <p class="empty">接続履歴はありません。</p>
  {:else}
    <div class="data-table history-table" role="table" aria-label="接続履歴">
      <div class="data-head" role="row">
        <span>アプリ</span><span>種類</span><span>結果</span><span>時刻</span>
      </div>
      {#each history as request (request.connection_id)}
        <div class="data-row" role="row">
          <strong>{request.name}</strong>
          <span class="muted small">接続</span>
          <span class="badge {request.status === 'completed' ? 'badge-ok' : 'badge-muted'}">
            {request.status === 'completed'
              ? '接続'
              : request.status === 'denied'
                ? '拒否'
                : '期限切れ'}
          </span>
          <span class="muted small">
            {formatDateTime(request.completed_at ?? request.approved_at ?? request.created_at)}
          </span>
        </div>
      {/each}
    </div>
  {/if}
</section>

<details class="panel management-panel">
  <summary>登録・権限</summary>
  <div class="management-body">
    <section>
      <h2>コードを確認</h2>
      <form
        onsubmit={(event) => {
          event.preventDefault();
          void review();
        }}
      >
        <label for="app-request">認可コードまたは登録コード</label>
        <input
          id="app-request"
          bind:value={input}
          oninput={() => {
            preview = null;
            consent = '';
            registered = '';
          }}
          placeholder="harmion-auth:v1:… または harmion-app:v1:…"
          autocomplete="off"
          required
        />
        <button class="primary" disabled={busy || !oauthEnabled}>
          {busy ? '確認中…' : '要求を確認'}
        </button>
      </form>

      {#if preview}
        <Confirm
          title={preview.type === 'registration'
            ? 'アプリを登録しますか？'
            : 'アプリへの権限を確認'}
          {busy}
          confirmLabel={preview.type === 'registration' ? '登録する' : '認可画面を準備'}
          onsubmit={approve}
          oncancel={() => (preview = null)}
        >
          <dl>
            <dt>アプリ名</dt>
            <dd>{preview.type === 'registration' ? preview.name : preview.preview.name}</dd>
            <dt>コールバック</dt>
            <dd class="mono">
              {preview.type === 'registration'
                ? preview.redirect_uri
                : preview.preview.redirect_uri}
            </dd>
            {#if preview.type === 'authorization'}
              <dt>Client ID</dt>
              <dd class="mono">{preview.preview.client_id}</dd>
              <dt>要求の有効期限</dt>
              <dd>{formatDateTime(preview.preview.expires_at)}</dd>
            {/if}
          </dl>
          {#if preview.type === 'authorization'}
            <h4>要求された権限</h4>
            <Grants grants={preview.preview.grants} dense />
          {/if}
        </Confirm>
      {/if}

      {#if registered}
        <label for="registered">要求元アプリへ渡す登録情報</label>
        <textarea id="registered" class="code-input" value={registered} readonly></textarea>
        <Clipboard text={registered} label="登録情報をコピー" />
      {/if}

      {#if consent}
        <a class="button primary" href={consent} target="_blank" rel="noopener noreferrer">
          Harmion の認可画面を開く ↗
        </a>
      {/if}
    </section>

    <section>
      <h2>権限を変更・失効</h2>
      <form
        onsubmit={(event) => {
          event.preventDefault();
          void change();
        }}
      >
        <label for="app-id">Client ID / App ID</label>
        <input id="app-id" bind:value={appId} placeholder="64桁の hex" required />
        <div class="grant-grid">
          <div>
            <label for="read-mode">読み取り</label>
            <select id="read-mode" bind:value={readMode}>
              <option value="none">なし</option>
              <option value="all">すべて</option>
              <option value="kinds">Kind 指定</option>
            </select>
            {#if readMode === 'kinds'}
              <input bind:value={readKinds} placeholder="Kind をカンマ区切り" />
            {/if}
          </div>
          <div>
            <label for="write-mode">書き込み</label>
            <select id="write-mode" bind:value={writeMode}>
              <option value="none">なし</option>
              <option value="all">すべて</option>
              <option value="kinds">Kind 指定</option>
            </select>
            {#if writeMode === 'kinds'}
              <input bind:value={writeKinds} placeholder="Kind をカンマ区切り" />
            {/if}
          </div>
        </div>
        <label class="check">
          <input type="checkbox" bind:checked={callGrant} />
          Interpreter を呼び出せる
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={provideGrant} />
          Interpreter を提供できる
        </label>
        {#if provideGrant}
          <label for="provide-names">Provider 名（空ならすべて）</label>
          <input id="provide-names" bind:value={provideNames} placeholder="wiki, mote" />
        {/if}
        <label class="check">
          <input type="checkbox" bind:checked={revoke} />
          アプリを失効させる（発行済みトークンも無効になります）
        </label>
        <button disabled={busy} class={revoke ? 'danger' : 'primary'}> 変更内容を確認 </button>
      </form>
    </section>
  </div>
</details>

<style>
  .app-columns {
    display: grid;
    grid-template-columns: minmax(0, 1.45fr) minmax(18rem, 1fr);
    gap: 1rem;
    align-items: start;
  }
  .app-cards,
  .provider-stack {
    display: grid;
    gap: 0.55rem;
    margin-top: 0.6rem;
  }
  .app-card,
  .provider-row {
    border: 1px solid var(--line);
    border-radius: 8px;
    background: #121721;
    padding: 0.8rem 0.9rem;
  }
  .app-card-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .app-card-head strong {
    font-size: 0.92rem;
  }
  .app-card p {
    margin: 0.3rem 0 0;
  }
  .provider-row {
    display: flex;
    align-items: start;
    justify-content: space-between;
    gap: 0.6rem;
    flex-wrap: wrap;
  }
  .provider-meta {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    flex-wrap: wrap;
  }
  .history-table .data-head,
  .history-table .data-row {
    grid-template-columns: minmax(9rem, 1.5fr) 5rem 6rem 9rem;
  }
  .management-panel {
    padding: 0;
    overflow: hidden;
  }
  .management-panel > summary {
    padding: 1rem 1.4rem;
    cursor: pointer;
    color: #b8c4da;
    font-size: 0.86rem;
  }
  .management-body {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 1.5rem;
    padding: 0 1.4rem 1.4rem;
    border-top: 1px solid var(--line);
  }
  .management-body h2 {
    margin-top: 1rem;
  }
  .grant-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.8rem;
  }
  @media (max-width: 900px) {
    .app-columns,
    .management-body {
      grid-template-columns: 1fr;
    }
  }
  @media (max-width: 650px) {
    .history-table .data-head,
    .history-table .data-row {
      grid-template-columns: minmax(8rem, 1.5fr) 5rem 6rem;
    }
    .history-table .data-head > :last-child,
    .history-table .data-row > :last-child {
      display: none;
    }
    .grant-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
