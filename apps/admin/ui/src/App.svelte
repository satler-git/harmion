<script lang="ts">
  import { onMount } from 'svelte';
  import {
    api,
    connect,
    endSession,
    errorMessage,
    loadCapabilities,
    detectMode,
    restoreWorkerSession,
    session,
    short,
    signInWorker,
    retrySave,
    workerAdmin,
    writes,
  } from './lib/api.svelte';
  import type { Overview, Page } from './lib/types';
  import OverviewPage from './pages/Overview.svelte';
  import Contents from './pages/Contents.svelte';
  import Conflicts from './pages/Conflicts.svelte';
  import Peers from './pages/Peers.svelte';
  import Apps from './pages/Apps.svelte';
  import Export from './pages/Export.svelte';
  import Interpreters from './pages/Interpreters.svelte';
  import Connections from './pages/Connections.svelte';
  const links: { id: Page; label: string; icon: string }[] = [
    { id: 'overview', label: '概要', icon: '◈' },
    { id: 'contents', label: 'コンテンツ', icon: '▤' },
    { id: 'conflicts', label: '競合解決', icon: '⇄' },
    { id: 'peers', label: 'Peer', icon: '⌘' },
    { id: 'apps', label: 'アプリ', icon: '▦' },
    { id: 'interpreters', label: 'Interpreter', icon: '⚙' },
    { id: 'connections', label: '接続要求', icon: '⇄' },
    { id: 'export', label: 'エクスポート', icon: '↗' },
  ];
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
    now = $state(Date.now());
  const capabilities = $derived(workerAdmin.capabilities);
  const visibleLinks = $derived(
    capabilities ? links.filter((link) => capabilities?.sections?.[link.id] !== false) : links,
  );
  const readOnly = $derived(worker || (capabilities?.read_only ?? false));
  const remaining = $derived(
    Math.min(30, Math.max(0, Math.ceil((session.expiresAt - now) / 60000))),
  );
  $effect(() => {
    if (!visibleLinks.some((link) => link.id === page)) page = 'overview';
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
      overview = await api<Overview>('/overview', {});
      sampledAt = new Date().toLocaleTimeString('ja-JP');
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  onMount(() => {
    let alive = true;
    const query = new URLSearchParams(location.search);
    const initial = query.get('page');
    if (links.some((link) => link.id === initial)) page = initial as Page;
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
  <title>Harmion · {visibleLinks.find((link) => link.id === page)?.label ?? '管理'}</title>
</svelte:head>
{#if !session.token}
  <div class="access-page">
    <div class="brand"><span class="brand-icon">H</span>Harmion</div>
    <section class="panel">
      {#if worker}
        <p class="eyebrow">REMOTE ACCESS</p>
        <h1>{opening ? '接続しています…' : 'Worker 管理画面に接続'}</h1>
        <p class="muted">
          {accessError ||
            'デプロイ時に発行された管理者トークンを入力してください。Cloudflare Access の背後でのみ利用できます。'}
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
        <p class="muted small">読み取り専用の管理画面です。承認・退役・同期は利用できません。</p>
      {:else}
        <p class="eyebrow">OWNER ACCESS</p>
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
      <div class="sidebar-label">管理コンソール</div>
      <nav aria-label="管理画面">
        {#each visibleLinks as link}<button
            class:active={page === link.id}
            aria-current={page === link.id ? 'page' : undefined}
            onclick={() => navigate(link.id)}
          >
            <span class="nav-icon" aria-hidden="true">{link.icon}</span>{link.label}
          </button>{/each}
      </nav>
      <div class="sidebar-footer">
        <span class="status-dot"></span>
        {worker ? 'Worker・読み取り専用' : 'ローカル接続'}
        <p class="mono">{overview ? short(overview.identity.node_id) : '状態を取得中…'}</p>
      </div>
    </aside>
    <div class="main-column">
      <header>
        <span>WORKSPACE <b>/</b> {visibleLinks.find((link) => link.id === page)?.label}</span>
        <div class="actions">
          <span class="badge">{readOnly ? '読み取り専用' : '所有者'}</span>
          {#if overview?.approval}
            <span
              class="badge {overview.approval.mode === 'session'
                ? 'badge-warn'
                : overview.approval.mode === 'deny' ||
                    (overview.approval.terminal_required && !overview.approval.terminal_available)
                  ? 'badge-danger'
                  : 'badge-info'}"
              title={overview.approval.mode === 'session'
                ? 'R2 はブラウザ確認だけで実行されます（--approval=session）'
                : overview.approval.mode === 'deny'
                  ? 'R2 は拒否する設定です（--approval=deny）'
                  : overview.approval.terminal_required && !overview.approval.terminal_available
                    ? '起動端末が TTY でないため、R2 は実行できません'
                    : 'R2 は起動したターミナルで y を押して承認します'}
            >
              {overview.approval.mode === 'session'
                ? '端末確認なし'
                : overview.approval.mode === 'deny'
                  ? 'R2 拒否'
                  : overview.approval.terminal_required && !overview.approval.terminal_available
                    ? '端末確認不可'
                    : '端末確認あり'}
            </span>
          {/if}
          {#if !worker}<span class="muted small">残り約 {remaining} 分</span>{/if}<button
            class="quiet"
            onclick={() => endSession()}
          >
            終了
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
          />
        </div>
        {#if !readOnly}
          <div class:hidden={page !== 'contents'}><Contents {resolve} /></div>
          <div class:hidden={page !== 'conflicts'}><Conflicts {requested} /></div>
        {/if}
        <div class:hidden={page !== 'peers'}>
          <Peers {overview} {refresh} active={page === 'peers'} {readOnly} />
        </div>
        {#if !readOnly}
          <div class:hidden={page !== 'apps'}><Apps {overview} /></div>
          <div class:hidden={page !== 'interpreters'}><Interpreters /></div>
          <div class:hidden={page !== 'connections'}>
            <Connections approval={overview?.approval ?? null} />
          </div>
          <div class:hidden={page !== 'export'}><Export /></div>
        {/if}
      </main>
      <footer>
        Harmion / {worker ? 'Worker replica (read-only)' : 'Local administration'}<span>
          {worker ? 'Cloudflare Access + 管理者トークン' : 'この端末の所有者セッション'}
        </span>
      </footer>
    </div>
  </div>
{/if}
