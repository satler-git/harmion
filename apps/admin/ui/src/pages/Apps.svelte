<script lang="ts">
  import { api, errorMessage } from '../lib/api.svelte';
  import type { AppPreview, Overview, Filter } from '../lib/types';
  import Confirm from '../lib/Confirm.svelte';
  import Clipboard from '../lib/Clipboard.svelte';
  import Grants from '../lib/Grants.svelte';
  import { formatDateTime } from '../lib/format';

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
    revoke = $state(false),
    changing = $state(false);

  const oauthEnabled = $derived(overview?.capabilities.oauth ?? false);

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
  function filter(mode: string, kinds: string): Filter | null {
    return mode === 'none'
      ? null
      : mode === 'all'
        ? { type: 'all' }
        : {
            type: 'kinds',
            kinds: [
              ...new Set(
                kinds
                  .split(/[,\n]/)
                  .map((kind) => kind.trim())
                  .filter(Boolean),
              ),
            ],
          };
  }
  function names(): string[] {
    return [
      ...new Set(
        provideNames
          .split(/[,\n]/)
          .map((name) => name.trim())
          .filter(Boolean),
      ),
    ];
  }
  async function change() {
    busy = true;
    error = '';
    try {
      await api('/change-grants', {
        app_id: appId.trim(),
        revoked: revoke,
        grants: {
          read: filter(readMode, readKinds),
          write: filter(writeMode, writeKinds),
          call: callGrant,
          provide: provideGrant,
          provide_names: provideGrant ? names() : [],
          manage_apps: false,
          manage_nodes: false,
        },
      });
      changing = false;
      message = revoke
        ? 'アプリを失効させ、指定した権限を反映しました。'
        : 'アプリの権限を置き換えました。';
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
      新しいアプリの初回接続は「接続要求」画面で行います。この画面は、アプリから渡された登録・認可コードの
      確認と、既存アプリの権限変更・失効に使います。
    </p>
  </div>
</div>

{#if !oauthEnabled}
  <p class="warning">
    このノードの OAuth は無効です。ノードの OAuth 設定を有効にしてからアプリを接続してください。
  </p>
{/if}
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if message}<p class="success" role="status">{message}</p>{/if}

<section class="panel">
  <div class="section-head">
    <div>
      <h2>登録・認可コードを確認</h2>
      <p class="muted small">
        アプリが表示したコードを貼り付けます。認可要求では、要求元アプリと権限を確認してから
        認可画面を開きます。
      </p>
    </div>
  </div>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      void review();
    }}
  >
    <label for="app-request">認可コードまたは登録コード</label>
    <input
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
      コードはアプリの認可画面からコピーしてください。要求された権限は次の確認画面に表示されます。
    </p>
    <button class="primary" disabled={busy || !oauthEnabled}>
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
        {#if preview.type === 'authorization'}
          <dt>Client ID</dt>
          <dd class="mono">{preview.preview.client_id}</dd>
          <dt>要求の有効期限</dt>
          <dd>{formatDateTime(preview.preview.expires_at)}</dd>
        {/if}
      </dl>
      {#if preview.type === 'authorization'}
        <h4>要求された権限</h4>
        <Grants grants={preview.preview.grants} dense />
      {/if}
      <p>
        {preview.type === 'registration'
          ? '登録した後にアプリが認可を要求します。登録情報は要求元アプリへ渡してください。'
          : '自分が開始したアプリの要求であることを確認してください。認可画面で許可すると、登録されたコールバックへ認可コードを返します。'}
      </p>
    </Confirm>
  {/if}

  {#if registered}
    <label for="registered">要求元アプリへ渡す登録情報</label>
    <textarea id="registered" class="code-input" value={registered} readonly></textarea>
    <Clipboard text={registered} label="登録情報をコピー" />
  {/if}

  {#if consent}
    <a class="button primary" href={consent} target="_blank" rel="noopener noreferrer">
      Harmion の認可画面を開く ↗
    </a>
    <p class="muted small">
      認可画面で許可すると、登録されたコールバックへ認可コードを返します。この画面は要求を
      準備しただけで、まだ権限は付与されていません。
    </p>
  {/if}
</section>

<section class="panel">
  <div class="section-head">
    <div>
      <h2>既存アプリの権限を変更・失効</h2>
      <p class="muted small">
        指定した権限でアプリの grant を置き換えます。現在の権限は自動では引き継がれません。 OAuth
        アプリでは権限の拡大は拒否され、縮小と失効だけが反映されます。
      </p>
    </div>
  </div>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      changing = true;
    }}
  >
    <label for="app-id">Client ID / App ID</label>
    <input
      id="app-id"
      bind:value={appId}
      oninput={() => (changing = false)}
      pattern={'[a-fA-F0-9]{64}'}
      placeholder="64桁の hex"
      required
    />
    <div class="two-columns">
      {#each ['read', 'write'] as op}
        <div>
          <label for={op + '-mode'}>{op === 'read' ? '読み取り' : '書き込み'}</label>
          {#if op === 'read'}
            <select id="read-mode" bind:value={readMode} onchange={() => (changing = false)}>
              <option value="none">なし</option>
              <option value="all">全 Content</option>
              <option value="kinds">指定 Kind</option>
            </select>
            {#if readMode === 'kinds'}
              <label for="read-kinds">Kind 名（カンマ区切り）</label>
              <input id="read-kinds" bind:value={readKinds} oninput={() => (changing = false)} />
            {/if}
          {:else}
            <select id="write-mode" bind:value={writeMode} onchange={() => (changing = false)}>
              <option value="none">なし</option>
              <option value="all">全 Content</option>
              <option value="kinds">指定 Kind</option>
            </select>
            {#if writeMode === 'kinds'}
              <label for="write-kinds">Kind 名（カンマ区切り）</label>
              <input id="write-kinds" bind:value={writeKinds} oninput={() => (changing = false)} />
            {/if}
          {/if}
        </div>
      {/each}
    </div>
    <label class="check">
      <input type="checkbox" bind:checked={callGrant} onchange={() => (changing = false)} />
      Interpreter を呼び出せる
    </label>
    <label class="check">
      <input type="checkbox" bind:checked={provideGrant} onchange={() => (changing = false)} />
      Interpreter を提供できる
    </label>
    {#if provideGrant}
      <label>
        提供を許可する Provider 名（カンマ区切り）
        <input
          bind:value={provideNames}
          oninput={() => (changing = false)}
          placeholder="ledger/balance, docs/search"
        />
      </label>
      <p class="muted small">
        空にすると「すべての Provider 名」を意味します。OAuth アプリでは既存より広い権限への
        変更は拒否されるため、通常は現在の名前を明示してください。
      </p>
    {/if}
    <label class="check">
      <input type="checkbox" bind:checked={revoke} onchange={() => (changing = false)} />
      アプリを失効させる（発行済みトークンも無効になります）
    </label>
    {#if revoke}
      <p class="warning small">
        失効は取り消せません。アプリを再び使うには、新しい接続または登録が必要です。
      </p>
    {/if}
    <p class="muted small">
      読み取り・書き込み・Interpreter 権限をまとめて置き換えます。管理用アプリ自身を指定すると
      管理画面が使えなくなる可能性があるため、対象の Client ID を確認してください。
    </p>
    <button disabled={busy} class={revoke ? 'danger' : 'primary'}> 変更内容を確認 </button>
  </form>

  {#if changing}
    <Confirm
      title={revoke ? 'アプリを失効' : 'アプリの権限を置き換え'}
      {busy}
      danger={revoke}
      onsubmit={change}
      oncancel={() => (changing = false)}
    >
      <p class="mono">{appId}</p>
      <p class="muted small">
        読み取り {readMode === 'kinds'
          ? readKinds || '指定なし'
          : readMode === 'none'
            ? 'なし'
            : '全 Content'}
        / 書き込み {writeMode === 'kinds'
          ? writeKinds || '指定なし'
          : writeMode === 'none'
            ? 'なし'
            : '全 Content'}
      </p>
      <h4>Interpreter</h4>
      <p class="small">
        {callGrant ? '呼び出し可' : '呼び出し不可'} · {provideGrant
          ? `提供可（${names().join('、') || 'すべての名前'}）`
          : '提供不可'}
      </p>
      {#if revoke}<p class="warning small">
          このアプリは失効し、発行済みトークンも無効になります。
        </p>{/if}
      {#if provideGrant && names().length === 0}
        <p class="warning small">
          Provider 名が空です。すべての名前を許可する要求として扱われます。
        </p>
      {/if}
      <p class="muted small">
        {revoke
          ? 'この操作は取り消せません。必要なら新しい接続を作り直してください。'
          : '現在の権限を置き換えます。意図しない縮小がないか確認してください。'}
      </p>
    </Confirm>
  {/if}
</section>

<style>
  .panel h4 {
    margin: 0.9rem 0 0.4rem;
    font-size: 0.85rem;
    color: #b9c6e6;
  }
</style>
