<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorMessage, short } from '../lib/api.svelte';
  import type { ContentItem, ContentPage } from '../lib/types';
  let { resolve }: { resolve: (id: string) => void } = $props();
  let items = $state<ContentItem[]>([]),
    next = $state<string | null>(null),
    selected = $state<ContentItem | null>(null);
  let filter = $state(''),
    busy = $state(false),
    error = $state(''),
    id = $state('');
  let filtered = $derived(
    items.filter((item) =>
      (item.body + ' ' + item.id + ' ' + item.kinds.join(' '))
        .toLocaleLowerCase()
        .includes(filter.toLocaleLowerCase()),
    ),
  );
  async function load(more = false) {
    busy = true;
    error = '';
    try {
      const page = await api<ContentPage>('/contents', { next: more ? next : null });
      items = more ? [...items, ...page.items] : page.items;
      next = page.next;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  onMount(() => {
    void load();
  });
</script>

<div class="page-title">
  <div>
    <p class="eyebrow">CONTENT BROWSER</p>
    <h1>コンテンツ</h1>
  </div>
  <button disabled={busy} onclick={() => load()}>一覧を更新</button>
</div>
<div class="browser-layout">
  <section class="panel content-list">
    <label for="content-filter">読み込んだ Content を検索</label><input
      id="content-filter"
      bind:value={filter}
      placeholder="本文・kind・ID で絞り込み"
    />
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <div class="list">
      {#each filtered as item (item.id)}
        <button class:chosen={selected?.id === item.id} onclick={() => (selected = item)}>
          <span>{item.preview.trim() || '本文なし'}</span><small>
            {short(item.id)}{#if item.conflicted}<b class="badge warn">競合あり</b>{/if}
          </small>
        </button>
      {/each}
    </div>
    {#if !items.length && !busy}<p class="empty">表示できる Content はありません。</p>{/if}
    {#if busy}<p class="muted">読み込み中…</p>{/if}
    {#if next}<button disabled={busy} onclick={() => load(true)}>続きを読み込む</button>{/if}
    <p class="muted small">
      {items.length} 件読み込み済み{next ? ' · ほかにも結果があります' : ''}
    </p>
    <hr />
    <label for="open-id">Content ID から競合解決を開く</label>
    <form
      onsubmit={(e) => {
        e.preventDefault();
        resolve(id.trim());
      }}
    >
      <input
        id="open-id"
        bind:value={id}
        pattern={'[a-fA-F0-9]{64}'}
        required
        placeholder="削除済み Content の ID も指定できます"
      /><button>開く</button>
    </form>
  </section>
  <section class="panel reader">
    {#if selected}
      <div class="page-title">
        <h2>本文</h2>
        <button onclick={() => resolve(selected!.id)}>競合解決で開く</button>
      </div>
      <p class="mono muted small">{selected.id}</p>
      <div class="tags">
        {#each selected.kinds as kind}<span class="badge">{kind}</span>{/each}
      </div>
      <pre class="body-text">{selected.body}</pre>
      <h3>属性</h3>
      <pre class="json-text">{selected.attributes}</pre>
    {:else}<div class="empty">
        <h2>Content を選択</h2>
        <p>左の一覧から本文と属性を確認できます。</p>
      </div>{/if}
  </section>
</div>
