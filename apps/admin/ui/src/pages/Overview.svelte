<script lang="ts">
  import type { Capabilities, Overview, Page } from '../lib/types';
  import { count } from '../lib/api.svelte';
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
    const n = BigInt(value);
    return n < 1048576n ? count(value) + ' B' : (Number(n / 1024n) / 1024).toFixed(1) + ' MiB';
  }
  const transport = $derived(
    overview?.identity.binding.transport === 'iroh'
      ? 'iroh'
      : overview?.identity.binding.transport === 'signed_https'
        ? 'Signed HTTPS'
        : 'HTTPS',
  );
</script>

<div class="page-title">
  <div>
    <p class="eyebrow">LOCAL NODE</p>
    <h1>ノードの概要</h1>
  </div>
  <button onclick={refresh} disabled={busy}>{busy ? '取得中…' : '最新の状態に更新'}</button>
</div>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if overview}
  {#if overview.metrics}
    <div class="stats">
      <article>
        <span>表示中の Content</span><strong>
          {count(overview.metrics.visible_contents)}
        </strong><small>削除済みを含む合計 {count(overview.metrics.contents)}</small>
      </article>
      <article>
        <span>保存した更新</span><strong>{count(overview.metrics.accepted_updates)}</strong><small>
          ローカルで受理した履歴
        </small>
      </article>
      <article>
        <span>履歴データ量</span><strong>{bytes(overview.metrics.record_bytes)}</strong><small>
          更新レコードの合計サイズ
        </small>
      </article>
      <article>
        <span>有効なアプリ登録</span><strong>{count(overview.metrics.active_apps)}</strong><small>
          所有者の管理用登録を含む
        </small>
      </article>
    </div>
  {:else}
    <div class="stats">
      <article>
        <span>管理モード</span><strong>読み取り専用</strong><small>Worker レプリカ</small>
      </article>
      <article>
        <span>Peer 登録</span><strong>{overview.peers?.length ?? 0}</strong><small>
          routing projection
        </small>
      </article>
      <article>
        <span>参加ノード</span><strong>{overview.members.members.length}</strong><small>
          Active {overview.members.members.filter((p) => p.state === 'active').length} 台
        </small>
      </article>
      <article>
        <span>Provisioning</span><strong>
          {overview.configured ? '設定済み' : '未設定'}
        </strong><small>デプロイ所有者の初期設定</small>
      </article>
    </div>
  {/if}
  <div class="two-columns">
    <section class="panel">
      <h2>このノード</h2>
      <dl>
        <dt>Node ID</dt>
        <dd class="mono">{overview.identity.node_id}</dd>
        <dt>Cluster ID</dt>
        <dd class="mono">{overview.identity.cluster_id}</dd>
        <dt>Peer 通信</dt>
        <dd>{transport}</dd>
        {#if overview.mode === 'worker' && overview.identity.binding.transport === 'signed_https'}<dt>
            Origin
          </dt>
          <dd class="mono">{(overview.identity.binding as { origin?: string }).origin}</dd>{/if}
        <dt>所有者権限</dt>
        <dd>
          {readOnly
            ? '読み取り専用（Peer 専用レプリカ）'
            : '全 Content の読み書き・アプリ管理・ノード管理'}
        </dd>
      </dl>
    </section>
    <section class="panel">
      <h2>接続と認可</h2>
      <dl>
        <dt>参加が有効なノード</dt>
        <dd>
          {overview.members.members.filter((p) => p.state === 'active').length} 台
        </dd>
        <dt>判断待ちの退役証明</dt>
        <dd>{overview.members.pending_retirements.length} 件</dd>
        <dt>OAuth</dt>
        <dd>{overview.capabilities.oauth ? '有効' : '無効'}</dd>
      </dl>
      <div class="actions">
        {#if capabilities?.sections?.peers !== false}<button onclick={() => navigate('peers')}>
            Peer を{readOnly ? '表示' : '管理'}
          </button>{/if}{#if !readOnly}<button onclick={() => navigate('apps')}>
            アプリを接続
          </button>{/if}
      </div>
    </section>
  </div>
  <p class="muted small">
    取得時刻 {sampledAt} · {readOnly
      ? 'この Worker レプリカで保持している範囲の状態です。App データ・Query・OAuth は提供していません。'
      : 'このノードで観測した状態です。履歴データ量には索引・WAL・ファイルの余白を含みません。'}
  </p>
{:else if busy}<p class="empty">ノードの状態を取得しています…</p>{/if}
