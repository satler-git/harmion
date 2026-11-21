<script lang="ts">
  import { api, errorMessage } from '../lib/api.svelte';
  import type { AppPreview, Overview, Grants, Filter } from '../lib/types';
  import Confirm from '../lib/Confirm.svelte';
  import Clipboard from '../lib/Clipboard.svelte';
  let { overview }: { overview: Overview | null } = $props();
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
    revoke = $state(true),
    changing = $state(false);
  function scopes(grants: Grants) {
    function filter(value: Filter | null) {
      return value === null ? 'なし' : value.type === 'all' ? '全 Content' : value.kinds.join('、');
    }
    return [
      { label: '読み取り', value: filter(grants.read) },
      { label: '書き込み', value: filter(grants.write) },
      { label: 'Interpreter 呼び出し', value: grants.call ? 'あり' : 'なし' },
      {
        label: 'Interpreter 提供',
        value: grants.provide
          ? grants.provide_names?.length
            ? grants.provide_names.join('、')
            : 'あり'
          : 'なし',
      },
    ];
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
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  async function change() {
    busy = true;
    error = '';
    const filter = (mode: string, kinds: string): Filter | null =>
      mode === 'none'
        ? null
        : mode === 'all'
          ? { type: 'all' }
          : {
              type: 'kinds',
              kinds: [
                ...new Set(
                  kinds
                    .split(/[,\n]/)
                    .map((k) => k.trim())
                    .filter(Boolean),
                ),
              ],
            };
    try {
      await api('/change-grants', {
        app_id: appId,
        revoked: revoke,
        grants: {
          read: filter(readMode, readKinds),
          write: filter(writeMode, writeKinds),
          call: callGrant,
          provide: provideGrant,
          provide_names: provideGrant
            ? provideNames
                .split(/[,\n]/)
                .map((name) => name.trim())
                .filter(Boolean)
            : [],
          manage_apps: false,
          manage_nodes: false,
        },
      });
      changing = false;
      message = 'アプリの権限を変更しました。';
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="page-title">
  <div>
    <p class="eyebrow">APPLICATION ACCESS</p>
    <h1>アプリ</h1>
    <p class="muted">
      アプリから渡された接続要求を、1行で貼り付けて確認します。初回接続とProvider登録は「接続」画面で1回の承認にまとめられます。
    </p>
  </div>
</div>
{#if !overview?.capabilities.oauth}<p class="warning">
    このノードの OAuth は無効です。ノードの OAuth 設定を有効にしてからアプリを接続してください。
  </p>{/if}
{#if error}<p class="error" role="alert">{error}</p>{/if}{#if message}<p
    class="success"
    role="status"
  >
    {message}
  </p>{/if}
<section class="panel">
  <h2>アプリの接続要求</h2>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      void review();
    }}
  >
    <label for="app-request">認可コードまたは登録コード</label><input
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
    <p class="muted">
      アプリの認可画面に表示されたコードをコピーしてください。要求された権限は次の画面で確認できます。
    </p>
    <button class="primary" disabled={busy || !overview?.capabilities.oauth}>
      {busy ? '確認中…' : '要求を確認'}
    </button>
  </form>
  {#if preview}
    <Confirm
      title={preview.type === 'registration' ? 'アプリを登録しますか？' : 'アプリへの権限を確認'}
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
          {preview.type === 'registration' ? preview.redirect_uri : preview.preview.redirect_uri}
        </dd>
        {#if preview.type === 'authorization'}<dt>Client ID</dt>
          <dd class="mono">{preview.preview.client_id}</dd>
          {#each scopes(preview.preview.grants) as scope}<dt>{scope.label}</dt>
            <dd>{scope.value}</dd>{/each}
          <dt>要求の有効期限</dt>
          <dd>
            {new Date(Number(BigInt(preview.preview.expires_at) / 1000n)).toLocaleString('ja-JP')}
          </dd>
        {/if}
      </dl>
      <p>
        {preview.type === 'registration'
          ? '登録した後にアプリが認可を要求します。'
          : '自分が開始したアプリの要求であることを確認してください。'}
      </p>
    </Confirm>
  {/if}
  {#if registered}<label for="registered">要求元アプリへ渡す登録情報</label><textarea
      id="registered"
      class="code-input"
      value={registered}
      readonly></textarea><Clipboard text={registered} label="登録情報をコピー" />{/if}
  {#if consent}<a class="button primary" href={consent} target="_blank" rel="noopener noreferrer">
      Harmion の認可画面を開く ↗
    </a>
    <p class="muted small">
      認可画面で許可すると、登録されたコールバックへ認可コードを返します。
    </p>{/if}
</section>
<section class="panel">
  <h2>アプリの権限を縮小・失効</h2>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      changing = true;
    }}
  >
    <label for="app-id">Client ID / App ID</label><input
      id="app-id"
      bind:value={appId}
      oninput={() => (changing = false)}
      pattern={'[a-fA-F0-9]{64}'}
      required
    />
    <div class="two-columns">
      {#each ['read', 'write'] as op}
        <div>
          <label for={op + '-mode'}>{op === 'read' ? '読み取り' : '書き込み'}</label>
          {#if op === 'read'}<select
              id="read-mode"
              bind:value={readMode}
              onchange={() => (changing = false)}
            >
              <option value="none">なし</option><option value="all">全 Content</option><option
                value="kinds"
              >
                指定 kind
              </option>
            </select>
            {#if readMode === 'kinds'}<label for="read-kinds">Kind 名（カンマ区切り）</label><input
                id="read-kinds"
                bind:value={readKinds}
                oninput={() => (changing = false)}
              />{/if}
          {:else}<select id="write-mode" bind:value={writeMode} onchange={() => (changing = false)}>
              <option value="none">なし</option><option value="all">全 Content</option><option
                value="kinds"
              >
                指定 kind
              </option>
            </select>
            {#if writeMode === 'kinds'}<label for="write-kinds">
                Kind 名（カンマ区切り）
              </label><input
                id="write-kinds"
                bind:value={writeKinds}
                oninput={() => (changing = false)}
              />{/if}
          {/if}
        </div>
      {/each}
    </div>
    <label class="check">
      <input
        type="checkbox"
        bind:checked={callGrant}
        onchange={() => (changing = false)}
      />Interpreter を呼び出せる
    </label>
    <label class="check">
      <input
        type="checkbox"
        bind:checked={provideGrant}
        onchange={() => (changing = false)}
      />Interpreter を提供できる
    </label>
    {#if provideGrant}
      <label>
        提供を許可する Provider 名（カンマ区切り。空なら全名前）
        <input
          bind:value={provideNames}
          oninput={() => (changing = false)}
          placeholder="ledger/balance, docs/search"
        />
      </label>
    {/if}
    <label class="check">
      <input
        type="checkbox"
        bind:checked={revoke}
        onchange={() => (changing = false)}
      />アプリを失効させる
    </label>
    <p class="muted small">
      OAuth の権限拡大には新しい認可要求が必要です。既存の名前付き提供を維持する場合は、同じ
      Provider 名を入力してください。空にすると全名前への拡大になり、拒否されます。
    </p>
    <button disabled={busy}>変更内容を確認</button>
  </form>
  {#if changing}<Confirm
      title="アプリの権限を変更"
      {busy}
      onsubmit={change}
      oncancel={() => (changing = false)}
    >
      <p class="mono">{appId}</p>
      <p>{revoke ? 'アプリを失効させます。' : '現在の認可範囲内で権限を変更します。'}</p>
      <p>
        読み取り {readMode}{readMode === 'kinds' ? ' · ' + readKinds : ''} / 書き込み {writeMode}{writeMode ===
        'kinds'
          ? ' · ' + writeKinds
          : ''} / Interpreter {callGrant ? '呼び出し可' : '呼び出し不可'} · {provideGrant
          ? `提供可（${provideNames.trim() || '全名前'}）`
          : '提供不可'}
      </p>
    </Confirm>{/if}
</section>
