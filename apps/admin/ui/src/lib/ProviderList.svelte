<script lang="ts">
  import type { ProviderManifest } from './types';
  import { defaultKinds, prettyJson } from './format';

  let {
    providers,
    replaceRequested = false,
    showSchemas = true,
  }: {
    providers: ProviderManifest[];
    replaceRequested?: boolean;
    showSchemas?: boolean;
  } = $props();
</script>

{#if providers.length === 0}
  <p class="muted small">この接続で登録される Provider はありません。</p>
{:else}
  <div class="provider-list">
    {#each providers as provider, index (provider.name + index)}
      <article class="provider-card">
        <div class="provider-head">
          <div>
            <strong>{provider.name}</strong>
            <p class="muted small">
              契約 v{provider.contract_version} · 既定の対象 Kind: {defaultKinds(provider)}
            </p>
          </div>
          {#if replaceRequested}
            <span class="badge badge-warn">同名を置換希望</span>
          {/if}
        </div>
        {#if showSchemas}
          <details>
            <summary>入出力スキーマを表示</summary>
            <div class="schema-grid">
              <div>
                <p class="meta-label">入力スキーマ</p>
                <pre class="json-text">{prettyJson(provider.input_schema)}</pre>
              </div>
              <div>
                <p class="meta-label">出力スキーマ</p>
                <pre class="json-text">{prettyJson(provider.output_schema)}</pre>
              </div>
            </div>
          </details>
        {/if}
      </article>
    {/each}
  </div>
{/if}

<style>
  .provider-list {
    display: grid;
    gap: 0.55rem;
  }
  .provider-card {
    background: #0c121d;
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 0.8rem 0.9rem;
  }
  .provider-head {
    display: flex;
    align-items: start;
    justify-content: space-between;
    gap: 0.75rem;
  }
  .provider-head strong {
    font-family: 'IBM Plex Mono', ui-monospace, monospace;
    font-size: 0.88rem;
    font-weight: 550;
  }
  .provider-head p {
    margin: 0.2rem 0 0;
  }
  details {
    margin-top: 0.55rem;
  }
  summary {
    color: var(--accent);
    cursor: pointer;
    font-size: 0.82rem;
  }
  .schema-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 18rem), 1fr));
    gap: 0.8rem;
    margin-top: 0.6rem;
  }
  .schema-grid .json-text {
    margin: 0;
    max-height: 18rem;
    overflow: auto;
    font-size: 0.75rem;
  }
</style>
