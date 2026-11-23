<script lang="ts">
  import type { Grants } from './types';
  import { grantRows } from './format';

  let { grants, dense = false }: { grants: Grants | null | undefined; dense?: boolean } = $props();
  const rows = $derived(grantRows(grants));
</script>

{#if !grants}
  <p class="muted small">権限情報を取得できませんでした。</p>
{:else}
  <dl class="grant-list" class:dense>
    {#each rows as row}
      <dt>{row.label}</dt>
      <dd class:muted={!row.active}>
        {#if row.active}<span class="dot ok" aria-hidden="true"></span>{/if}{row.value}
      </dd>
    {/each}
  </dl>
{/if}

<style>
  .grant-list {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 0.35rem 1rem;
    margin: 0;
  }
  .grant-list.dense {
    gap: 0.15rem 0.8rem;
  }
  .grant-list dt {
    color: var(--muted);
    font-size: 0.8rem;
  }
  .grant-list dd {
    margin: 0;
    font-size: 0.88rem;
    overflow-wrap: anywhere;
  }
  .dot {
    display: inline-block;
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: #6f7e9d;
    margin-right: 0.45rem;
    vertical-align: 0.1em;
  }
  .dot.ok {
    background: #83d7b1;
  }
</style>
