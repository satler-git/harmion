<script lang="ts">
  import { api, errorMessage } from '../lib/api.svelte';
  import Confirm from '../lib/Confirm.svelte';
  import Clipboard from '../lib/Clipboard.svelte';

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
    <p class="muted">
      本文・現在値・未解決の候補・削除記録を、管理アプリを起動した端末の新しいディレクトリへ保存します。
    </p>
  </div>
</div>

<section class="panel narrow">
  <h2>出力先を指定</h2>
  <p class="muted">
    既存ディレクトリは上書きしません。存在しない新しいパスを指定してください。出力には
    所有者専用の権限が設定されます。
  </p>
  <div class="export-scope">
    <div>
      <span class="meta-label">含まれるもの</span>
      <p>本文ファイル、現在値、未解決の候補、削除記録、metadata.jsonl、manifest.json</p>
    </div>
    <div>
      <span class="meta-label">含まれないもの</span>
      <p>ノード鍵・管理資格情報・OAuth 秘密。復旧にはノードバックアップの手順を使います。</p>
    </div>
  </div>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      review = true;
    }}
  >
    <label for="export-path">新規ディレクトリ</label>
    <input
      id="export-path"
      bind:value={path}
      oninput={() => (review = false)}
      required
      placeholder="/home/you/exports/harmion-2026-11-20"
    />
    <button class="primary" disabled={busy}>出力内容を確認</button>
  </form>

  {#if review}
    <Confirm
      title="Export を実行"
      {busy}
      confirmLabel="出力する"
      onsubmit={run}
      oncancel={() => (review = false)}
    >
      <p class="mono">{path}</p>
      <p>
        所有者専用の権限で新しいディレクトリを作成し、本文とメタデータを書き出します。
        完了後は本文ファイル、metadata.jsonl、manifest.json を確認できます。
      </p>
    </Confirm>
  {/if}

  {#if busy}
    <p role="status">データを出力しています。この画面を開いたままお待ちください。</p>
  {/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if done}
    <p class="success" role="status">Export が完了しました。</p>
    <div class="done-path">
      <p class="mono">{done}</p>
      <Clipboard text={done} label="パスをコピー" />
    </div>
  {/if}
</section>

<style>
  .export-scope {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.8rem;
    margin: 1rem 0;
  }
  .export-scope > div {
    border: 1px solid #2a3448;
    border-radius: 8px;
    background: #0e131c;
    padding: 0.75rem 0.85rem;
  }
  .export-scope p {
    margin: 0.3rem 0 0;
    color: var(--muted);
    font-size: 0.84rem;
  }
  .done-path {
    display: flex;
    align-items: center;
    gap: 0.7rem;
    flex-wrap: wrap;
  }
  .done-path p {
    margin: 0;
  }
  @media (max-width: 730px) {
    .export-scope {
      grid-template-columns: 1fr;
    }
  }
</style>
