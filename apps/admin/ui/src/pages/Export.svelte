<script lang="ts">
  import { api, errorMessage } from '../lib/api.svelte';
  import Confirm from '../lib/Confirm.svelte';
  let path = $state(''),
    review = $state(false),
    busy = $state(false),
    error = $state(''),
    done = $state('');
  async function run() {
    busy = true;
    error = '';
    done = '';
    try {
      await api('/export', { directory: path });
      done = path;
      review = false;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="page-title">
  <div>
    <p class="eyebrow">DATA PORTABILITY</p>
    <h1>Export</h1>
    <p class="muted">本文・現在値・未解決の候補・削除記録をファイルに保存します。</p>
  </div>
</div>
<section class="panel narrow">
  <h2>出力先を指定</h2>
  <p class="muted">管理アプリを起動した端末上の、新しいディレクトリを指定してください。</p>
  <form
    onsubmit={(e) => {
      e.preventDefault();
      review = true;
    }}
  >
    <label for="export-path">新規ディレクトリ</label><input
      id="export-path"
      bind:value={path}
      oninput={() => (review = false)}
      required
      placeholder="/home/you/exports/harmion-2026-11-20"
    /><button class="primary" disabled={busy}>出力内容を確認</button>
  </form>
  {#if review}<Confirm
      title="Export を実行"
      {busy}
      onsubmit={run}
      oncancel={() => (review = false)}
    >
      <p class="mono">{path}</p>
      <p>
        所有者専用の権限で作成します。完了後は本文ファイルと metadata.jsonl、manifest.json
        を確認できます。
      </p>
    </Confirm>{/if}
  {#if busy}<p role="status">データを出力しています。この画面を開いたままお待ちください。</p>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}{#if done}<p
      class="success"
      role="status"
    >
      Export が完了しました。
    </p>
    <p class="mono">{done}</p>{/if}
  <p class="muted small">
    出力範囲は現在値・未解決の候補・削除記録です。ノード鍵と資格情報の復旧にはノードバックアップの手順を使います。
  </p>
</section>
