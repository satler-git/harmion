<script lang="ts">
  import { onMount } from 'svelte';
  import {
    api,
    connect,
    endSession,
    errorMessage,
    session,
    short,
    retrySave,
    writes,
  } from './lib/api.svelte';
  import type { Overview, Page } from './lib/types';
  import OverviewPage from './pages/Overview.svelte';
  import Contents from './pages/Contents.svelte';
  import Conflicts from './pages/Conflicts.svelte';
  import Peers from './pages/Peers.svelte';
  import Apps from './pages/Apps.svelte';
  import Export from './pages/Export.svelte';
  const links: { id: Page; label: string; icon: string }[] = [
    { id: 'overview', label: '概要', icon: '◈' },
    { id: 'contents', label: 'コンテンツ', icon: '▤' },
    { id: 'conflicts', label: '競合解決', icon: '⇄' },
    { id: 'peers', label: 'Peer', icon: '⌘' },
    { id: 'apps', label: 'アプリ', icon: '▦' },
    { id: 'export', label: 'Export', icon: '↗' },
  ];
  let page = $state<Page>('overview'),
    overview = $state<Overview | null>(null),
    busy = $state(false),
    error = $state(''),
    sampledAt = $state('');
  let opening = $state(true),
    accessError = $state(''),
    requested = $state(''),
    now = $state(Date.now());
  const remaining = $derived(
    Math.min(30, Math.max(0, Math.ceil((session.expiresAt - now) / 60000))),
  );
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
        const connected = await connect();
        if (connected && alive) await refresh();
      } catch (e) {
        accessError = errorMessage(e);
      } finally {
        if (alive) opening = false;
      }
    })();
    const interval = setInterval(() => {
      now = Date.now();
      if (session.token && now >= session.expiresAt) {
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
  <title>Harmion · {links.find((link) => link.id === page)?.label ?? '管理'}</title>
</svelte:head>
{#if !session.token}
  <div class="access-page">
    <div class="brand"><span class="brand-icon">H</span>Harmion</div>
    <section class="panel">
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
      <pre>harmion-admin run NODE_DIR UI_DIR</pre>
      <p class="muted small">リンクは2分、所有者セッションは30分有効です。</p>
    </section>
  </div>
{:else}
  <div class="app-shell">
    <aside class="sidebar">
      <div class="brand"><span class="brand-icon">H</span>Harmion</div>
      <div class="sidebar-label">管理コンソール</div>
      <nav aria-label="管理画面">
        {#each links as link}<button
            class:active={page === link.id}
            aria-current={page === link.id ? 'page' : undefined}
            onclick={() => navigate(link.id)}
          >
            <span class="nav-icon" aria-hidden="true">{link.icon}</span>{link.label}
          </button>{/each}
      </nav>
      <div class="sidebar-footer">
        <span class="status-dot"></span> ローカル接続
        <p class="mono">{overview ? short(overview.identity.node_id) : '状態を取得中…'}</p>
      </div>
    </aside>
    <div class="main-column">
      <header>
        <span>WORKSPACE <b>/</b> {links.find((link) => link.id === page)?.label}</span>
        <div class="actions">
          <span class="badge">所有者</span><span class="muted small">
            残り約 {remaining} 分
          </span><button class="quiet" onclick={() => endSession()}>終了</button>
        </div>
      </header>
      {#if writes.pending}<div class="save-banner" role="status">
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
          <OverviewPage {overview} {sampledAt} {refresh} {busy} {error} {navigate} />
        </div>
        <div class:hidden={page !== 'contents'}><Contents {resolve} /></div>
        <div class:hidden={page !== 'conflicts'}><Conflicts {requested} /></div>
        <div class:hidden={page !== 'peers'}>
          <Peers {overview} {refresh} active={page === 'peers'} />
        </div>
        <div class:hidden={page !== 'apps'}><Apps {overview} /></div>
        <div class:hidden={page !== 'export'}><Export /></div>
      </main>
      <footer>Harmion / Local administration<span>この端末の所有者セッション</span></footer>
    </div>
  </div>
{/if}
