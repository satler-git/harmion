<script lang="ts">
  import type { Capabilities, Overview, Page } from '../lib/types';
  import { count } from '../lib/api.svelte';
  import Clipboard from '../lib/Clipboard.svelte';
  import Grants from '../lib/Grants.svelte';
  import { bindingTarget, transportLabel } from '../lib/format';
  import { approvalModeLabel, approvalTone } from '../lib/format';

  let {
    overview,
    capabilities,
    sampledAt,
    refresh,
    busy,
    error,
    navigate,
    readOnly,
  }: {
    overview: Overview | null;
    capabilities: Capabilities | null;
    sampledAt: string;
    refresh: () => void;
    busy: boolean;
    error: string;
    navigate: (page: Page) => void;
    readOnly: boolean;
  } = $props();

  function bytes(value: string) {
    try {
      const n = BigInt(value);
      return n < 1048576n ? count(value) + ' B' : (Number(n / 1024n) / 1024).toFixed(1) + ' MiB';
    } catch {
      return '—';
    }
  }
  const workerMode = $derived(overview?.mode === 'worker' || capabilities?.mode === 'worker');
  const activeMembers = $derived(
    overview?.members.members.filter((member) => member.state === 'active').length ?? 0,
  );
  const oauthOrigin = $derived(
    (capabilities as (Capabilities & { oauth_origin?: string | null }) | null)?.oauth_origin ??
      null,
  );
  const binding = $derived(overview?.identity.binding ?? null);
  const bindingDetail = $derived(
    binding
      ? binding.transport === 'signed_https'
        ? binding.origin
        : binding.transport === 'iroh'
          ? (binding.relay_url ?? binding.endpoint_id)
          : binding.endpoint
      : null,
  );
</script>

<div class="page-title">
  <div>
    <p class="eyebrow">{workerMode ? 'REMOTE REPLICA' : 'LOCAL NODE'}</p>
    <h1>ノードの概要</h1>
    <p class="muted">
      {workerMode
        ? 'Peer 専用の読み取り専用レプリカです。表示できる範囲の状態だけを返します。'
        : 'このノードの同一性、保存量、参加ノード、認可状態をまとめて確認します。'}
    </p>
  </div>
  <button disabled={busy} onclick={refresh}>{busy ? '取得中…' : '最新の状態に更新'}</button>
</div>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if overview}
  {#if overview.metrics}
    <div class="stats">
      <article>
        <span>表示中の Content</span><strong>{count(overview.metrics.visible_contents)}</strong>
        <small>削除済みを含む合計 {count(overview.metrics.contents)}</small>
      </article>
      <article>
        <span>保存した更新</span><strong>{count(overview.metrics.accepted_updates)}</strong>
        <small>ローカルで受理した履歴</small>
      </article>
      <article>
        <span>履歴データ量</span><strong>{bytes(overview.metrics.record_bytes)}</strong>
        <small>更新レコードの合計サイズ</small>
      </article>
      <article>
        <span>有効なアプリ登録</span><strong>{count(overview.metrics.active_apps)}</strong>
        <small>所有者の管理用登録を含む</small>
      </article>
    </div>
  {:else}
    <div class="stats">
      <article>
        <span>管理モード</span><strong>読み取り専用</strong><small>Worker レプリカ</small>
      </article>
      <article>
        <span>Peer 登録</span><strong>{overview.peers?.length ?? 0}</strong>
        <small>routing projection</small>
      </article>
      <article>
        <span>参加ノード</span><strong>{overview.members.members.length}</strong>
        <small>有効 {activeMembers} 台</small>
      </article>
      <article>
        <span>Provisioning</span><strong>{overview.configured ? '設定済み' : '未設定'}</strong>
        <small>デプロイ所有者の初期設定</small>
      </article>
    </div>
  {/if}

  <div class="two-columns">
    <section class="panel">
      <div class="section-head">
        <div>
          <h2>このノード</h2>
          <p class="muted small">Peer とアプリが接続するときの識別情報です。</p>
        </div>
        <span class="badge {readOnly ? 'badge-muted' : 'badge-ok'}">
          {readOnly ? '読み取り専用' : '所有者'}
        </span>
      </div>
      <dl>
        <dt>Node ID</dt>
        <dd class="mono id-line">
          <span>{overview.identity.node_id}</span>
          <Clipboard text={overview.identity.node_id} label="コピー" />
        </dd>
        <dt>Cluster ID</dt>
        <dd class="mono id-line">
          <span>{overview.identity.cluster_id}</span>
          <Clipboard text={overview.identity.cluster_id} label="コピー" />
        </dd>
        <dt>Peer 通信</dt>
        <dd>
          {binding ? transportLabel(binding.transport) : '—'}
          {#if bindingDetail}<span class="mono muted small binding-detail">
              {bindingDetail}
            </span>{/if}
        </dd>
        {#if binding?.transport === 'signed_https' && binding.signing_key}
          <dt>署名公開鍵</dt>
          <dd class="mono small">{binding.signing_key}</dd>
        {/if}
        <dt>所有者権限</dt>
        <dd>
          {readOnly
            ? '読み取り専用（Peer 専用レプリカ）'
            : '全 Content の読み書き・アプリ管理・ノード管理'}
        </dd>
        {#if overview.permissions}
          <dt>管理セッションの grant</dt>
          <dd><Grants grants={overview.permissions} dense /></dd>
        {/if}
      </dl>
    </section>

    <section class="panel">
      <div class="section-head">
        <div>
          <h2>接続と認可</h2>
          <p class="muted small">クラスタ参加とアプリ接続の入口です。</p>
        </div>
      </div>
      <dl>
        <dt>参加が有効なノード</dt>
        <dd>{activeMembers} 台</dd>
        <dt>判断待ちの退役証明</dt>
        <dd>{overview.members.pending_retirements.length} 件</dd>
        <dt>OAuth</dt>
        <dd>
          {overview.capabilities.oauth ? '有効' : '無効'}
          {#if oauthOrigin}<span class="mono muted small binding-detail">{oauthOrigin}</span>{/if}
        </dd>
        {#if overview.approval}
          <dt>端末承認</dt>
          <dd>
            <span class="badge {approvalTone(overview.approval)}">
              {overview.approval.mode}
            </span>
            <span class="small"> {approvalModeLabel(overview.approval)}</span>
          </dd>
        {/if}
        {#if overview.sync_status}
          <dt>同期状態</dt>
          <dd class="small">同期情報を取得しました（詳細は Peer 画面で確認してください）。</dd>
        {/if}
      </dl>
      <div class="actions">
        {#if capabilities?.sections?.peers !== false}
          <button onclick={() => navigate('peers')}>Peer を{readOnly ? '表示' : '管理'}</button>
        {/if}
        {#if !readOnly && capabilities?.sections?.connections !== false}
          <button onclick={() => navigate('connections')}>接続要求を見る</button>
        {/if}
        {#if !readOnly && capabilities?.sections?.interpreters !== false}
          <button onclick={() => navigate('interpreters')}>Interpreter 提供元</button>
        {/if}
        {#if !readOnly && capabilities?.sections?.apps !== false}
          <button class="primary" onclick={() => navigate('apps')}>アプリを接続</button>
        {/if}
      </div>
    </section>
  </div>

  <p class="muted small">
    取得時刻 {sampledAt || '—'} · {readOnly
      ? 'この Worker レプリカで保持している範囲の状態です。App データ・Query・OAuth は提供していません。'
      : 'このノードで観測した状態です。履歴データ量には索引・WAL・ファイルの余白を含みません。'}
  </p>
{:else if busy}
  <p class="empty">ノードの状態を取得しています…</p>
{/if}

<style>
  .id-line {
    display: flex;
    align-items: start;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .id-line > span {
    min-width: 0;
  }
  .id-line :global(button) {
    padding: 0.15rem 0.45rem;
    font-size: 0.72rem;
    flex-shrink: 0;
  }
  .binding-detail {
    display: block;
    margin-top: 0.2rem;
  }
  .section-head {
    margin-bottom: 0.4rem;
  }
  .section-head + dl {
    margin-top: 0.8rem;
  }
  .panel dl :global(.grant-list) {
    margin: 0;
  }
</style>
