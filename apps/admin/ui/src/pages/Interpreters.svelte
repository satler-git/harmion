<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorMessage, short } from '../lib/api.svelte';
  import type { ApprovalStatus, InterpreterDescription, InterpreterProvider } from '../lib/types';
  import Confirm from '../lib/Confirm.svelte';
  import {
    interpreterStatusLabel,
    interpreterStatusTone,
    prettyJson,
    shortId,
  } from '../lib/format';

  let {
    embedded = false,
    approval = null,
    onchange,
  }: {
    embedded?: boolean;
    approval?: ApprovalStatus | null;
    onchange?: () => void;
  } = $props();
  let providers = $state<InterpreterProvider[]>([]);
  let descriptions = $state<Record<string, InterpreterDescription>>({});
  let busy = $state(false),
    error = $state(''),
    message = $state('');
  let pending = $state<InterpreterProvider | null>(null),
    replace = $state(false);

  const waiting = $derived(providers.filter((provider) => provider.status === 'pending'));
  const approved = $derived(providers.filter((provider) => provider.status === 'approved'));
  const approvalBlocked = $derived(
    approval?.mode === 'deny' ||
      (approval?.terminal_required === true && approval.terminal_available === false),
  );
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
      message = `「${pending.name}」を承認しました。`;
      pending = null;
      replace = false;
      await load();
      onchange?.();
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

{#if !embedded}
  <div class="page-title">
    <div>
      <h1>Provider</h1>
    </div>
    <button class="quiet" disabled={busy} onclick={load}>{busy ? '更新中…' : '再読み込み'}</button>
  </div>
{/if}

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if message}<p class="success" role="status">{message}</p>{/if}
{#if approvalBlocked}
  <div class="callout danger">この起動方法では承認できません。</div>
{/if}

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
                <button
                  class="primary"
                  disabled={busy || approvalBlocked}
                  onclick={() => openApproval(provider)}
                >
                  承認
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

{#if pending}
  <Confirm
    title="Interpreter 提供元を承認"
    {busy}
    disabled={approvalBlocked}
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
    <p class="muted small">同名の提供元がある場合は置換が必要です。</p>
    {#if approval && approval.mode === 'terminal' && approval.terminal_available}
      <p class="muted small">
        承認すると、起動したターミナルで <kbd>y</kbd> を押します。
      </p>
    {:else if approval && approval.mode === 'session'}
      <p class="muted small">この起動方法では端末確認を行いません。</p>
    {/if}
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
  .confirm-grid {
    grid-template-columns: 8rem minmax(0, 1fr);
    gap: 0.35rem 1rem;
  }
  @media (max-width: 730px) {
    .provider-head {
      flex-direction: column;
    }
    .confirm-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
