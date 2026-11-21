<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorMessage, short } from '../lib/api.svelte';
  import type { InterpreterDescription, InterpreterProvider } from '../lib/types';
  import Confirm from '../lib/Confirm.svelte';

  let providers = $state<InterpreterProvider[]>([]);
  let descriptions = $state<Record<string, InterpreterDescription>>({});
  let busy = $state(false),
    error = $state(''),
    message = $state('');
  let pending = $state<InterpreterProvider | null>(null),
    replace = $state(false);

  function statusLabel(status: InterpreterProvider['status']) {
    return status === 'approved' ? '承認済み' : status === 'pending' ? '承認待ち' : '置換済み';
  }
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
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  onMount(load);
</script>

<div class="page-title">
  <div>
    <p class="eyebrow">LOCAL INTERPRETER</p>
    <h1>Interpreter</h1>
    <p class="muted">
      このノードに登録された計算提供元を確認し、未承認の登録を承認します。呼び出しはアプリが App API
      の call で行い、この画面は計算を実行しません。
    </p>
  </div>
  <button class="quiet" disabled={busy} onclick={load}>再読み込み</button>
</div>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if message}<p class="success" role="status">{message}</p>{/if}
<section class="panel">
  <h2>登録済みの提供元</h2>
  {#if providers.length === 0}
    <p class="empty">登録された Interpreter 提供元はありません。</p>
  {:else}
    <div class="provider-list">
      {#each providers as provider (provider.provider_id)}
        <article class="provider-row">
          <div>
            <h3>{provider.name}</h3>
            <p class="mono small">{short(provider.provider_id)}</p>
            <p class="muted small">
              契約 v{provider.contract_version} · {provider.status === 'approved'
                ? '承認済み'
                : provider.status === 'pending'
                  ? '承認待ち'
                  : '置換済み'} · {provider.connected ? '接続中' : '未接続'}
            </p>
          </div>
          <div class="actions">
            {#if provider.status === 'pending'}
              <button
                class="primary"
                disabled={busy}
                onclick={() => {
                  pending = provider;
                  replace = false;
                }}
              >
                承認を確認
              </button>
            {/if}
            <button class="quiet" disabled={busy} onclick={() => describe(provider)}>
              定義を表示
            </button>
          </div>
          {#if descriptions[provider.provider_id]}
            <pre class="json-text">{JSON.stringify(
                descriptions[provider.provider_id],
                null,
                2,
              )}</pre>
          {/if}
        </article>
      {/each}
    </div>
  {/if}
</section>
<section class="panel">
  <h2>接続と権限の扱い</h2>
  <p class="muted small">
    提供元アプリは provide 権限を持ち、ノードへ認証付き WebSocket
    を開いて自身を登録します。承認済みの
    名前はノード内で一つです。同じ名前を別の提供元へ切り替えるときは、下の確認で「置換」を明示します。
    承認と現在の接続状態は別であり、未接続の提供元は呼び出し時に利用不能になります。
  </p>
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
    <dl>
      <dt>名前</dt>
      <dd>{pending.name}</dd>
      <dt>Provider ID</dt>
      <dd class="mono">{pending.provider_id}</dd>
      <dt>契約バージョン</dt>
      <dd>{pending.contract_version}</dd>
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
    gap: 12px;
  }
  .provider-row {
    border: 1px solid var(--line, #30343c);
    border-radius: 10px;
    padding: 14px 16px;
    display: grid;
    gap: 10px;
  }
  .provider-row h3 {
    margin: 0 0 4px;
    font-size: 1rem;
  }
  .provider-row .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .json-text {
    margin: 0;
    max-height: 360px;
    overflow: auto;
  }
</style>
