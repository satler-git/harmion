<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorMessage, short } from '../lib/api.svelte';
  import type { ContentItem, ContentPage } from '../lib/types';
  import Clipboard from '../lib/Clipboard.svelte';
  import { prettyJson } from '../lib/format';

  let { resolve }: { resolve: (id: string) => void } = $props();
  let items = $state<ContentItem[]>([]),
    next = $state<string | null>(null),
    selected = $state<ContentItem | null>(null);
  let filter = $state(''),
    busy = $state(false),
    error = $state(''),
    id = $state('');

  interface ContentAttribute {
    namespace: string;
    key: string;
    value: unknown;
  }

  const filtered = $derived(
    items.filter((item) =>
      (item.body + ' ' + item.id + ' ' + item.kinds.join(' '))
        .toLocaleLowerCase()
        .includes(filter.toLocaleLowerCase()),
    ),
  );
  const selectedAttributes = $derived(selected ? parseAttributes(selected.attributes) : []);

  function parseAttributes(raw: string): ContentAttribute[] {
    try {
      const value: unknown = JSON.parse(raw);
      if (!Array.isArray(value)) return [];
      return value.filter(
        (entry): entry is ContentAttribute =>
          typeof entry === 'object' &&
          entry !== null &&
          typeof (entry as ContentAttribute).namespace === 'string' &&
          typeof (entry as ContentAttribute).key === 'string',
      );
    } catch {
      return [];
    }
  }
  function attributeValue(value: unknown): string {
    return typeof value === 'string' ? value : prettyJson(value);
  }
  async function load(more = false) {
    busy = true;
    error = '';
    try {
      const page = await api<ContentPage>('/contents', { next: more ? next : null });
      items = more ? [...items, ...page.items] : page.items;
      next = page.next;
      if (!more) selected = null;
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
    <h1>Content</h1>
  </div>
  <button disabled={busy} onclick={() => load()}>{busy ? '読み込み中…' : '一覧を更新'}</button>
</div>

<div class="browser-layout">
  <section class="panel content-list">
    <div class="section-head">
      <div>
        <h2>一覧</h2>
        <p class="muted small">
          読み込んだ {items.length} 件の中を検索します。サーバー全体の検索ではありません。
        </p>
      </div>
      <span class="badge">{filtered.length} / {items.length}</span>
    </div>
    <label for="content-filter">読み込んだ Content を絞り込み</label>
    <input id="content-filter" bind:value={filter} placeholder="本文・Kind・ID で検索" />
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if busy && items.length === 0}
      <p class="empty">Content を読み込んでいます…</p>
    {:else if items.length === 0}
      <p class="empty">表示できる Content はありません。</p>
    {:else if filtered.length === 0}
      <p class="empty">「{filter}」に一致する Content はありません。</p>
    {:else}
      <div class="list">
        {#each filtered as item (item.id)}
          <button class:chosen={selected?.id === item.id} onclick={() => (selected = item)}>
            <span>{item.preview.trim() || '本文なし'}</span>
            <small class="list-meta">
              <span class="mono">{short(item.id)}</span>
              {#if item.kinds.length}
                {#each item.kinds.slice(0, 3) as kind}<span class="badge">{kind}</span>{/each}
                {#if item.kinds.length > 3}<span class="badge badge-muted">
                    +{item.kinds.length - 3}
                  </span>{/if}
              {/if}
              {#if item.conflicted}<span class="badge badge-warn">競合あり</span>{/if}
            </small>
          </button>
        {/each}
      </div>
    {/if}
    {#if busy && items.length > 0}<p class="muted small">更新中…</p>{/if}
    {#if next}
      <button disabled={busy} onclick={() => load(true)}>続きを読み込む</button>
    {/if}
    <p class="muted small">
      {items.length} 件読み込み済み{next ? ' · さらに結果があります' : ' · 最後まで読み込みました'}
    </p>
    <hr />
    <label for="open-id">Content ID から競合解決を開く</label>
    <form
      onsubmit={(event) => {
        event.preventDefault();
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
        <div>
          <p class="muted small">表示しているのは現在値です。</p>
        </div>
        <button onclick={() => resolve(selected!.id)}>競合解決で開く</button>
      </div>
      <div class="id-line">
        <p class="mono muted small">{selected.id}</p>
        <Clipboard text={selected.id} label="ID をコピー" />
      </div>
      <div class="tags">
        {#each selected.kinds as kind}<span class="badge">{kind}</span>{/each}
        {#if selected.conflicted}<span class="badge badge-warn">競合あり</span>{/if}
        {#if selected.kinds.length === 0 && !selected.conflicted}<span class="muted small">
            Kind なし
          </span>{/if}
      </div>
      <h3>本文</h3>
      <pre class="body-text">{selected.body || '（本文なし）'}</pre>
      <h3>属性</h3>
      {#if selectedAttributes.length}
        <dl class="attribute-list">
          {#each selectedAttributes as attribute}
            <dt class="mono small">{attribute.namespace} / {attribute.key}</dt>
            <dd><pre class="json-text">{attributeValue(attribute.value)}</pre></dd>
          {/each}
        </dl>
      {:else}
        <p class="muted small">属性はありません。</p>
      {/if}
    {:else}
      <div class="empty">
        <h2>Content を選択</h2>
      </div>
    {/if}
  </section>
</div>

<style>
  .list-meta {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    flex-wrap: wrap;
  }
  .reader .tags {
    margin: 0.7rem 0 1rem;
    flex-wrap: wrap;
  }
  .reader h3 {
    margin-top: 1.2rem;
    font-size: 0.9rem;
    color: #b9c6e6;
  }
  .id-line {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    flex-wrap: wrap;
  }
  .id-line p {
    margin: 0;
  }
  .id-line :global(button) {
    padding: 0.2rem 0.5rem;
    font-size: 0.75rem;
  }
  .attribute-list {
    grid-template-columns: minmax(8rem, 14rem) minmax(0, 1fr);
    gap: 0.5rem 1rem;
    margin: 0.5rem 0 0;
  }
  .attribute-list dd {
    margin: 0;
  }
  .attribute-list .json-text {
    margin: 0;
    max-height: 18rem;
    overflow: auto;
  }
  @media (max-width: 730px) {
    .attribute-list {
      grid-template-columns: 1fr;
    }
  }
</style>
