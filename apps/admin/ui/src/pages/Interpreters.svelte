<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorMessage, short } from '../lib/api.svelte';
  import type { InterpreterDescription, InterpreterProvider } from '../lib/types';
  import Confirm from '../lib/Confirm.svelte';
  import {
    interpreterStatusLabel,
    interpreterStatusTone,
    prettyJson,
    shortId,
  } from '../lib/format';

  let providers = $state<InterpreterProvider[]>([]);
  let descriptions = $state<Record<string, InterpreterDescription>>({});
  let busy = $state(false),
    error = $state(''),
    message = $state('');
  let pending = $state<InterpreterProvider | null>(null),
    replace = $state(false);

  const waiting = $derived(providers.filter((provider) => provider.status === 'pending'));
  const approved = $derived(providers.filter((provider) => provider.status === 'approved'));
  const superseded = $derived(providers.filter((provider) => provider.status === 'superseded'));

  async function load() {
    busy = true;
    error = '';
    try {
      const result = await api<{ providers: InterpreterProvider[] }>('/interpreter-list', {});
      providers = result.providers;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  async function describe(provider: InterpreterProvider) {
    if (descriptions[provider.provider_id]) {
      const next = { ...descriptions };
      delete next[provider.provider_id];
      descriptions = next;
      return;
    }
    busy = true;
    error = '';
    try {
      const result = await api<InterpreterDescription>('/interpreter-describe', {
        provider: provider.provider_id,
      });
      descriptions = { ...descriptions, [provider.provider_id]: result };
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  async function approve() {
    if (!pending) return;
    busy = true;
    error = '';
    try {
      await api('/interpreter-approve', {
        provider: pending.provider_id,
        replace,
      });
      message = `「${pending.name}」を承認しました。アプリが接続すると呼び出し可能になります。`;
      pending = null;
      replace = false;
      await load();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  function openApproval(provider: InterpreterProvider) {
    pending = provider;
    replace = false;
  }
  onMount(load);
</script>

<div class="page-title">
  <div>
    <h1>Interpreter 提供元</h1>
  </div>
  <button class="quiet" disabled={busy} onclick={load}>{busy ? '更新中…' : '再読み込み'}</button>
</div>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if message}<p class="success" role="status">{message}</p>{/if}

{#if providers.length === 0}
  <section class="panel">
    <div class="empty">
      <h2>登録された提供元はありません</h2>
    </div>
  </section>
{:else}
  {#if waiting.length}
    <section class="panel">
      <div class="section-head">
        <div>
          <h2>承認待ち</h2>
          <p class="muted small">
            名前・契約バージョン・スキーマを確認してください。同名の承認済み提供元がある場合は、
            置換を明示したときだけ承認できます。
          </p>
        </div>
        <span class="badge badge-warn">{waiting.length} 件</span>
      </div>
      <div class="provider-list">
        {#each waiting as provider (provider.provider_id)}
          <article class="provider-card pending">
            <div class="provider-head">
              <div>
                <div class="title-line">
                  <h3>{provider.name}</h3>
                  <span class="badge {interpreterStatusTone(provider.status)}">
                    {interpreterStatusLabel(provider.status)}
                  </span>
                  <span class="badge {provider.connected ? 'badge-ok' : 'badge-muted'}">
                    {provider.connected ? '接続中' : '接続前'}
                  </span>
                </div>
                <p class="mono muted small">{short(provider.provider_id)}</p>
              </div>
              <div class="actions">
                <button class="primary" disabled={busy} onclick={() => openApproval(provider)}>
                  承認内容を確認
                </button>
                <button class="quiet" disabled={busy} onclick={() => describe(provider)}>
                  {descriptions[provider.provider_id] ? '定義を閉じる' : '定義を表示'}
                </button>
              </div>
            </div>
            <p class="muted small">契約 v{provider.contract_version}</p>
            {#if descriptions[provider.provider_id]}
              <pre class="json-text">{prettyJson(descriptions[provider.provider_id])}</pre>
            {/if}
          </article>
        {/each}
      </div>
    </section>
  {/if}

  <section class="panel">
    <div class="section-head">
      <div>
        <h2>承認済み</h2>
        <p class="muted small">
          「接続中」は提供元アプリが現在 WebSocket を開いている状態です。未接続の提供元は
          呼び出し時に使えません。
        </p>
      </div>
      <span class="badge badge-ok">{approved.length} 件</span>
    </div>
    {#if approved.length === 0}
      <p class="empty">承認済みの提供元はありません。</p>
    {:else}
      <div class="provider-list">
        {#each approved as provider (provider.provider_id)}
          <article class="provider-card">
            <div class="provider-head">
              <div>
                <div class="title-line">
                  <h3>{provider.name}</h3>
                  <span class="badge {interpreterStatusTone(provider.status)}">
                    {interpreterStatusLabel(provider.status)}
                  </span>
                  <span class="badge {provider.connected ? 'badge-ok' : 'badge-muted'}">
                    {provider.connected ? '接続中' : '未接続'}
                  </span>
                </div>
                <p class="mono muted small">{short(provider.provider_id)}</p>
              </div>
              <div class="actions">
                <button class="quiet" disabled={busy} onclick={() => describe(provider)}>
                  {descriptions[provider.provider_id] ? '定義を閉じる' : '定義を表示'}
                </button>
              </div>
            </div>
            <p class="muted small">
              契約 v{provider.contract_version} · Provider ID {shortId(provider.provider_id, 8, 6)}
            </p>
            {#if descriptions[provider.provider_id]}
              <pre class="json-text">{prettyJson(descriptions[provider.provider_id])}</pre>
            {/if}
          </article>
        {/each}
      </div>
    {/if}
  </section>

  {#if superseded.length}
    <section class="panel">
      <div class="section-head">
        <div>
          <h2>置換済み</h2>
          <p class="muted small">
            同じ名前の新しい提供元に置き換えられた登録です。呼び出しには使われません。
          </p>
        </div>
        <span class="badge badge-muted">{superseded.length} 件</span>
      </div>
      <div class="provider-list">
        {#each superseded as provider (provider.provider_id)}
          <article class="provider-card muted-card">
            <div class="provider-head">
              <div>
                <div class="title-line">
                  <h3>{provider.name}</h3>
                  <span class="badge {interpreterStatusTone(provider.status)}">
                    {interpreterStatusLabel(provider.status)}
                  </span>
                </div>
                <p class="mono muted small">{short(provider.provider_id)}</p>
              </div>
              <button class="quiet" disabled={busy} onclick={() => describe(provider)}>
                {descriptions[provider.provider_id] ? '定義を閉じる' : '定義を表示'}
              </button>
            </div>
            {#if descriptions[provider.provider_id]}
              <pre class="json-text">{prettyJson(descriptions[provider.provider_id])}</pre>
            {/if}
          </article>
        {/each}
      </div>
    </section>
  {/if}
{/if}

<section class="panel">
  <h2>承認と接続の違い</h2>
  <dl class="explain-grid">
    <dt>承認</dt>
    <dd>
      このノードが Provider 名を信頼した状態です。統合接続の承認、またはこの画面の承認で確定します。
    </dd>
    <dt>接続</dt>
    <dd>
      提供元アプリが認証付き WebSocket を開いている状態です。ノード再起動やアプリ終了で失われます。
    </dd>
    <dt>同名の置換</dt>
    <dd>
      ノード内で承認済みの名前は1つだけです。別の提供元へ切り替えるときは、承認時に「置換」を明示します。
    </dd>
  </dl>
</section>

{#if pending}
  <Confirm
    title="Interpreter 提供元を承認"
    {busy}
    confirmLabel="承認する"
    onsubmit={approve}
    oncancel={() => {
      pending = null;
      replace = false;
    }}
  >
    <dl class="confirm-grid">
      <dt>名前</dt>
      <dd>{pending.name}</dd>
      <dt>Provider ID</dt>
      <dd class="mono">{pending.provider_id}</dd>
      <dt>契約バージョン</dt>
      <dd>v{pending.contract_version}</dd>
    </dl>
    <label class="check">
      <input type="checkbox" bind:checked={replace} />
      同じ名前の承認済み提供元を置換する
    </label>
    <p class="muted small">
      置換を選ばない場合、同名の承認済み提供元があれば承認は失敗します。置換すると以前の提供元の
      セッションは閉じられ、その名前の呼び出しは新しい提供元へ切り替わります。
    </p>
  </Confirm>
{/if}

<style>
  .provider-list {
    display: grid;
    gap: 0.7rem;
  }
  .provider-card {
    border: 1px solid var(--line);
    border-radius: 10px;
    padding: 0.9rem 1rem;
    display: grid;
    gap: 0.55rem;
    background: #0f141e;
  }
  .provider-card.pending {
    border-color: #5a4630;
    background: linear-gradient(180deg, #181711 0%, #0f141e 7rem);
  }
  .muted-card {
    opacity: 0.8;
  }
  .provider-head {
    display: flex;
    align-items: start;
    justify-content: space-between;
    gap: 1rem;
  }
  .title-line {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .title-line h3 {
    margin: 0;
    font-size: 0.98rem;
    font-family: 'IBM Plex Mono', ui-monospace, monospace;
    font-weight: 550;
  }
  .provider-head > div:first-child {
    min-width: 0;
  }
  .provider-head p {
    margin: 0.25rem 0 0;
  }
  .provider-card > p {
    margin: 0;
  }
  .json-text {
    margin: 0;
    max-height: 24rem;
    overflow: auto;
  }
  .explain-grid {
    grid-template-columns: 7rem minmax(0, 1fr);
    gap: 0.5rem 1rem;
    margin: 0;
  }
  .explain-grid dt {
    font-weight: 600;
    color: #b9c6e6;
  }
  .confirm-grid {
    grid-template-columns: 8rem minmax(0, 1fr);
    gap: 0.35rem 1rem;
  }
  @media (max-width: 730px) {
    .provider-head {
      flex-direction: column;
    }
    .explain-grid,
    .confirm-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
