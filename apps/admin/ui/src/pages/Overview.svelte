<script lang="ts">
  import type { Overview, Page } from '../lib/types';
  import { count } from '../lib/api.svelte';
  let {
    overview,
    sampledAt,
    refresh,
    busy,
    error,
    navigate,
  }: {
    overview: Overview | null;
    sampledAt: string;
    refresh: () => void;
    busy: boolean;
    error: string;
    navigate: (page: Page) => void;
  } = $props();
  function bytes(value: string) {
    const n = BigInt(value);
    return n < 1048576n ? count(value) + ' B' : (Number(n / 1024n) / 1024).toFixed(1) + ' MiB';
  }
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
  <div class="two-columns">
    <section class="panel">
      <h2>このノード</h2>
      <dl>
        <dt>Node ID</dt>
        <dd class="mono">{overview.identity.node_id}</dd>
        <dt>Cluster ID</dt>
        <dd class="mono">{overview.identity.cluster_id}</dd>
        <dt>Peer 通信</dt>
        <dd>{overview.identity.binding.transport === 'iroh' ? 'iroh' : 'HTTPS'}</dd>
        <dt>所有者権限</dt>
        <dd>全 Content の読み書き・アプリ管理・ノード管理</dd>
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
        <button onclick={() => navigate('peers')}>Peer を管理</button><button
          onclick={() => navigate('apps')}
        >
          アプリを接続
        </button>
      </div>
    </section>
  </div>
  <p class="muted small">
    取得時刻 {sampledAt} · このノードで観測した状態です。履歴データ量には索引・WAL・ファイルの余白を含みません。
  </p>
{:else if busy}<p class="empty">ノードの状態を取得しています…</p>{/if}
