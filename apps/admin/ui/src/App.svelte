<script lang="ts">
  import { onMount } from 'svelte';
  import {
    api,
    connect,
    endSession,
    errorMessage,
    loadCapabilities,
    loadWorkerStatus,
    detectMode,
    restoreWorkerSession,
    session,
    short,
    signInWorker,
    retrySave,
    workerAdmin,
    writes,
  } from './lib/api.svelte';
  import type { ConnectionList, InterpreterProvider, Overview, Page } from './lib/types';
  import OverviewPage from './pages/Overview.svelte';
  import Contents from './pages/Contents.svelte';
  import Conflicts from './pages/Conflicts.svelte';
  import Peers from './pages/Peers.svelte';
  import Apps from './pages/Apps.svelte';
  import Export from './pages/Export.svelte';
  import Approvals from './pages/Approvals.svelte';
  import Icon from './lib/Icon.svelte';
  const links: {
    id: Page;
    label: string;
    icon: 'overview' | 'approvals' | 'nodes' | 'content' | 'apps' | 'export';
  }[] = [
    { id: 'overview', label: '概要', icon: 'overview' },
    { id: 'approvals', label: '承認', icon: 'approvals' },
    { id: 'peers', label: 'ノード', icon: 'nodes' },
    { id: 'contents', label: 'Content', icon: 'content' },
    { id: 'apps', label: 'アプリ', icon: 'apps' },
    { id: 'export', label: 'Export', icon: 'export' },
  ];
  const labels: Partial<Record<Page, string>> = { conflicts: '競合解決' };
  const routes: Page[] = [...links.map((link) => link.id), 'conflicts'];
  let page = $state<Page>('overview'),
    overview = $state<Overview | null>(null),
    busy = $state(false),
    error = $state(''),
    sampledAt = $state('');
  let opening = $state(true),
    accessError = $state(''),
    requested = $state(''),
    tokenInput = $state(''),
    worker = $state(false),
    approvalCount = $state(0),
    now = $state(Date.now());
  const capabilities = $derived(workerAdmin.capabilities);
  const readOnly = $derived(worker || (capabilities?.read_only ?? false));
  const visibleLinks = $derived.by(() => {
    if (readOnly) return links.filter((link) => link.id === 'overview' || link.id === 'peers');
    return capabilities
      ? links.filter((link) => capabilities.sections?.[link.id] !== false)
      : links;
  });
  const remainingClock = $derived.by(() => {
    const total = Math.max(0, Math.ceil((session.expiresAt - now) / 1000));
    return `${String(Math.floor(total / 60)).padStart(2, '0')}:${String(total % 60).padStart(2, '0')}`;
  });
  $effect(() => {
    if (page !== 'conflicts' && !visibleLinks.some((link) => link.id === page)) page = 'overview';
  });
  async function authenticateWorker() {
    opening = true;
    accessError = '';
    try {
      await signInWorker(tokenInput);
      tokenInput = '';
      await refresh();
    } catch (e) {
      accessError = errorMessage(e);
    } finally {
      opening = false;
    }
  }
  function navigate(next: Page) {
    page = next;
    history.replaceState(null, '', '?page=' + next + (requested ? '&content=' + requested : ''));
  }
  function resolve(id: string) {
    requested = id;
    page = 'conflicts';
    history.replaceState(null, '', '?page=conflicts&content=' + encodeURIComponent(id));
  }
  async function refresh() {
    busy = true;
    error = '';
    try {
      const [nextOverview] = await Promise.all([
        api<Overview>(worker ? '/admin/overview' : '/overview', {}),
        worker ? loadWorkerStatus() : Promise.resolve(null),
      ]);
      overview = nextOverview;
      sampledAt = new Date().toLocaleTimeString('ja-JP');
      if (!worker) await refreshApprovalCount();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  async function refreshApprovalCount() {
    const [connections, providers] = await Promise.all([
      api<ConnectionList>('/connect-list', {}).catch(() => null),
      api<{ providers: InterpreterProvider[] }>('/interpreter-list', {}).catch(() => null),
    ]);
    approvalCount =
      (connections?.connections.filter((request) => request.status === 'pending').length ?? 0) +
      (providers?.providers.filter((provider) => provider.status === 'pending').length ?? 0);
  }
  onMount(() => {
    let alive = true;
    const query = new URLSearchParams(location.search);
    const initial = query.get('page');
    const target = initial === 'connections' || initial === 'interpreters' ? 'approvals' : initial;
    if (target && routes.includes(target as Page)) page = target as Page;
    const content = query.get('content');
    if (content) {
      requested = content;
      page = 'conflicts';
    }
    void (async () => {
      try {
        worker = (await detectMode()) === 'worker';
        if (worker) {
          const saved = restoreWorkerSession();
          if (saved) {
            await signInWorker(saved).catch(() => {});
          }
          if (alive && session.token) await refresh();
          return;
        }
        const connected = await connect();
        if (connected && alive) {
          await loadCapabilities();
          await refresh();
        }
      } catch (e) {
        accessError = errorMessage(e);
      } finally {
        if (alive) opening = false;
      }
    })();
    const interval = setInterval(() => {
      now = Date.now();
      if (session.mode === 'native' && session.token && now >= session.expiresAt) {
        session.token = '';
        session.ended = true;
      }
    }, 1000);
    return () => {
      alive = false;
      clearInterval(interval);
    };
  });
</script>

<svelte:head>
  <title
    >Harmion · {visibleLinks.find((link) => link.id === page)?.label ??
      labels[page] ??
      '管理'}</title
  >
</svelte:head>
{#if !session.token}
  <div class="access-page">
    <div class="brand"><span class="brand-icon">H</span>Harmion</div>
    <section class="panel">
      {#if worker}
        <h1>{opening ? '接続しています…' : 'Worker 管理画面に接続'}</h1>
        <p class="muted">
          {accessError || 'デプロイ時に発行された管理者トークンを入力してください。'}
        </p>
        <input
          type="password"
          bind:value={tokenInput}
          placeholder="管理者トークン"
          disabled={opening}
          onkeydown={(event) => {
            if (event.key === 'Enter' && tokenInput.trim()) void authenticateWorker();
          }}
        />
        <button
          class="primary"
          disabled={opening || !tokenInput.trim()}
          onclick={authenticateWorker}
        >
          {opening ? '確認中…' : '接続'}
        </button>
      {:else}
        <h1>
          {opening
            ? '接続しています…'
            : session.ended
              ? 'セッションが終了しました'
              : '管理画面を開く'}
        </h1>
        <p class="muted">
          {accessError ||
            'harmion-admin の起動時に表示された単回リンクを開いてください。リンクが使用済み・期限切れの場合は管理アプリを再起動してください。'}
        </p>
        <pre>harmion-admin run NODE_DIR</pre>
        <p class="muted small">リンクは2分、所有者セッションは30分有効です。</p>
      {/if}
    </section>
  </div>
{:else}
  <div class="app-shell">
    <aside class="sidebar">
      <div class="brand"><span class="brand-icon">H</span>Harmion</div>
      <nav aria-label="管理画面">
        {#each visibleLinks as link}<button
            class:active={page === link.id || (link.id === 'contents' && page === 'conflicts')}
            class:nav-export={link.id === 'export'}
            aria-current={page === link.id || (link.id === 'contents' && page === 'conflicts')
              ? 'page'
              : undefined}
            onclick={() => navigate(link.id)}
          >
            <span class="nav-icon">
              <Icon name={link.icon} size={17} />
            </span>{link.label}{#if link.id === 'approvals' && approvalCount > 0}<span
                class="nav-count"
              >
                {approvalCount}
              </span>{/if}
          </button>{/each}
      </nav>
    </aside>
    <div class="main-column">
      <header>
        <div class="topbar-context">
          <span class="status-dot"></span>
          <span>{worker ? 'Worker' : 'ローカルノード'}</span>
          <span class="mono muted">{overview ? short(overview.identity.node_id) : '—'}</span>
        </div>
        <div class="actions">
          <span class="badge">{readOnly ? '読み取り専用' : '所有者'}</span>
          {#if overview?.approval && (overview.approval.mode === 'deny' || (overview.approval.terminal_required && !overview.approval.terminal_available))}
            <span
              class="badge badge-danger"
              title="起動端末で承認できないため、R2 操作は実行できません"
            >
              端末承認不可
            </span>
          {:else if overview?.approval?.mode === 'session'}
            <span class="badge badge-warn" title="ブラウザ確認だけで R2 を実行します">
              端末確認なし
            </span>
          {:else if overview?.approval?.mode === 'terminal' && overview.approval.terminal_available}
            <span class="badge badge-info" title="承認時に起動端末で y を押します">端末承認</span>
          {/if}
          {#if !worker}
            <span class="topbar-clock">残り <strong>{remainingClock}</strong></span>
          {/if}
          <button
            class="icon-button"
            aria-label="セッションを終了"
            title="セッションを終了"
            onclick={() => endSession()}
          >
            <Icon name="power" size={17} />
          </button>
        </div>
      </header>
      {#if !readOnly && writes.pending}<div class="save-banner" role="status">
          <span>
            {writes.busy
              ? '保存中…'
              : '保存結果を確認できていません。別の操作に進む前に同じ要求で確認してください。'}
          </span><button disabled={writes.busy} onclick={() => retrySave().catch(() => {})}>
            同じ要求で再試行
          </button>{#if writes.error}<p>{writes.error}</p>{/if}
        </div>{/if}
      <main>
        <div class:hidden={page !== 'overview'}>
          <OverviewPage
            {overview}
            {capabilities}
            {sampledAt}
            {refresh}
            {busy}
            {error}
            {navigate}
            {readOnly}
            {now}
            active={page === 'overview'}
            {approvalCount}
          />
        </div>
        {#if !readOnly}
          <div class:hidden={page !== 'approvals'}>
            <Approvals
              approval={overview?.approval ?? null}
              oncount={(count) => (approvalCount = count)}
            />
          </div>
          <div class:hidden={page !== 'contents'}><Contents {resolve} /></div>
          <div class:hidden={page !== 'conflicts'}><Conflicts {requested} /></div>
        {/if}
        <div class:hidden={page !== 'peers'}>
          <Peers {overview} {refresh} active={page === 'peers'} {readOnly} />
        </div>
        {#if !readOnly}
          <div class:hidden={page !== 'apps'}><Apps {overview} {navigate} /></div>
          <div class:hidden={page !== 'export'}><Export /></div>
        {/if}
      </main>
    </div>
  </div>
{/if}
