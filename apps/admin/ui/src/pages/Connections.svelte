<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorMessage, short } from '../lib/api.svelte';
  import type { ConnectionList, ConnectionRequest } from '../lib/types';
  import Confirm from '../lib/Confirm.svelte';

  let connections = $state<ConnectionRequest[]>([]);
  let busy = $state(false),
    error = $state(''),
    message = $state('');
  let decision = $state<{ request: ConnectionRequest; allow: boolean } | null>(null),
    replace = $state(false);

  function statusLabel(status: ConnectionRequest['status']) {
    switch (status) {
      case 'pending':
        return '承認待ち';
      case 'approved':
        return '承認済み';
      case 'denied':
        return '拒否';
      case 'completed':
        return '接続完了';
      case 'expired':
        return '期限切れ';
    }
  }
  function filterName(value: ConnectionRequest['grants']['read']) {
    if (value === null) return 'なし';
    return value.type === 'all' ? '全 Content' : value.kinds.join('、');
  }
  function providerNames(request: ConnectionRequest) {
    const names = request.grants.provide_names ?? [];
    return names.length ? names.join('、') : request.grants.provide ? '指定なし' : 'なし';
  }
  async function load() {
    busy = true;
    error = '';
    try {
      connections = (await api<ConnectionList>('/connect-list', {})).connections;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
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
        replace,
      });
      message = decision.allow
        ? '接続を承認しました。アプリが完了すると接続状態になります。'
        : '接続要求を拒否しました。';
      decision = null;
      replace = false;
      await load();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  onMount(() => {
    void load();
    const timer = setInterval(() => {
      if (!busy && !decision) void load();
    }, 3000);
    return () => clearInterval(timer);
  });
</script>

<div class="page-title">
  <div>
    <p class="eyebrow">INTEGRATED CONNECTION</p>
    <h1>接続</h1>
    <p class="muted">
      アプリと Interpreter 提供元の接続要求を、1回の承認でまとめて許可します。アプリは承認後に一時
      secret と PKCE でトークンを受け取り、Provider は同じ承認で登録されます。
    </p>
  </div>
  <button class="quiet" disabled={busy} onclick={load}>再読み込み</button>
</div>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if message}<p class="success" role="status">{message}</p>{/if}
<section class="panel">
  <h2>接続要求</h2>
  {#if connections.length === 0}
    <p class="empty">保留中または最近の接続要求はありません。</p>
  {:else}
    <div class="connection-list">
      {#each connections as request (request.connection_id)}
        <article class="connection-row">
          <div class="connection-head">
            <div>
              <h3>{request.name}</h3>
              <p class="mono small">{short(request.connection_id)}</p>
            </div>
            <span class="badge" class:badge-warn={request.status === 'pending'}>
              {statusLabel(request.status)}
            </span>
          </div>
          <dl class="connection-grid">
            <dt>Callback</dt>
            <dd class="mono small">{request.redirect_uri}</dd>
            <dt>Web origin</dt>
            <dd class="mono small">{request.web_origin ?? 'なし'}</dd>
            <dt>読み取り</dt>
            <dd>{filterName(request.grants.read)}</dd>
            <dt>書き込み</dt>
            <dd>{filterName(request.grants.write)}</dd>
            <dt>Interpreter 呼び出し</dt>
            <dd>{request.grants.call ? 'あり' : 'なし'}</dd>
            <dt>Interpreter 提供</dt>
            <dd>{providerNames(request)}</dd>
            <dt>Provider</dt>
            <dd>
              {request.provider
                ? `${request.provider.name}（契約 v${request.provider.contract_version}）`
                : 'なし'}
            </dd>
            <dt>期限</dt>
            <dd>{new Date(Number(request.expires_at) / 1000).toLocaleString('ja-JP')}</dd>
          </dl>
          {#if request.provider}
            <details>
              <summary>Provider 定義</summary>
              <pre class="json-text">{JSON.stringify(
                  {
                    input_schema: request.provider.input_schema,
                    output_schema: request.provider.output_schema,
                    default_query: request.provider.default_query,
                  },
                  null,
                  2,
                )}</pre>
            </details>
          {/if}
          {#if request.status === 'pending'}
            <div class="actions">
              <button
                class="primary"
                disabled={busy}
                onclick={() => {
                  decision = { request, allow: true };
                  replace = false;
                }}
              >
                承認を確認
              </button>
              <button
                class="danger"
                disabled={busy}
                onclick={() => {
                  decision = { request, allow: false };
                }}
              >
                拒否
              </button>
            </div>
          {:else if request.status === 'approved'}
            <p class="muted small">
              アプリの完了待ちです。完了するとトークンと Provider ticket が発行されます。
            </p>
          {/if}
        </article>
      {/each}
    </div>
  {/if}
</section>
{#if decision}
  <Confirm
    title={decision.allow ? '接続を承認' : '接続を拒否'}
    {busy}
    confirmLabel={decision.allow ? '承認する' : '拒否する'}
    onsubmit={decide}
    oncancel={() => {
      decision = null;
      replace = false;
    }}
  >
    <p>{decision.request.name} の接続要求です。</p>
    <p class="mono small">{decision.request.connection_id}</p>
    {#if decision.allow && decision.request.provider}
      {#if decision.request.provider_replace}
        <p class="muted small">
          アプリは同名 Provider の置換を希望しています。置換する場合だけ次を選んでください。
        </p>
      {/if}
      <label class="check">
        <input type="checkbox" bind:checked={replace} />
        同名の承認済み Provider を置換する
      </label>
    {/if}
    {#if decision.allow}
      <p class="muted small">
        アプリ grant の確定、Provider の登録・承認、トークン発行の準備を1つの操作で行います。
      </p>
    {:else}
      <p class="muted small">この要求には資格情報も Provider も発行されません。</p>
    {/if}
  </Confirm>
{/if}

<style>
  .connection-list {
    display: grid;
    gap: 14px;
  }
  .connection-row {
    border: 1px solid var(--line, #30343c);
    border-radius: 10px;
    padding: 14px 16px;
    display: grid;
    gap: 10px;
  }
  .connection-head {
    display: flex;
    align-items: start;
    justify-content: space-between;
    gap: 12px;
  }
  .connection-head h3 {
    margin: 0 0 4px;
    font-size: 1rem;
  }
  .badge-warn {
    border-color: var(--warning, #a97a2a);
    color: var(--warning, #a97a2a);
  }
  .connection-grid {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 4px 14px;
    margin: 0;
  }
  .connection-grid dt {
    color: var(--muted, #8d939e);
  }
  .connection-grid dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .json-text {
    margin: 0;
    max-height: 360px;
    overflow: auto;
  }
</style>
