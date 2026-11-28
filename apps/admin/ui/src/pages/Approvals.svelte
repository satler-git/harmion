<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../lib/api.svelte';
  import type { ApprovalStatus, ConnectionList, InterpreterProvider } from '../lib/types';
  import Connections from './Connections.svelte';
  import Interpreters from './Interpreters.svelte';

  let {
    approval = null,
    oncount,
  }: {
    approval?: ApprovalStatus | null;
    oncount?: (count: number) => void;
  } = $props();
  let tab = $state<'connections' | 'providers'>('connections');
  let connections = $state(0),
    providers = $state(0),
    generation = $state(0);

  async function loadCounts() {
    const [connectionList, providerList] = await Promise.all([
      api<ConnectionList>('/connect-list', {}).catch(() => null),
      api<{ providers: InterpreterProvider[] }>('/interpreter-list', {}).catch(() => null),
    ]);
    connections =
      connectionList?.connections.filter((request) => request.status === 'pending').length ?? 0;
    providers =
      providerList?.providers.filter((provider) => provider.status === 'pending').length ?? 0;
    oncount?.(connections + providers);
  }

  function reload() {
    generation += 1;
    void loadCounts();
  }

  onMount(loadCounts);
</script>

<div class="page-title">
  <div>
    <h1>承認</h1>
  </div>
  <button class="quiet" onclick={reload}>更新</button>
</div>

<div class="segmented approval-kinds" role="tablist" aria-label="承認の種類">
  <button
    role="tab"
    aria-selected={tab === 'connections'}
    class:active={tab === 'connections'}
    onclick={() => (tab = 'connections')}
  >
    接続 <span class="count">{connections}</span>
  </button>
  <button
    role="tab"
    aria-selected={tab === 'providers'}
    class:active={tab === 'providers'}
    onclick={() => (tab = 'providers')}
  >
    Provider <span class="count">{providers}</span>
  </button>
</div>

{#key generation}
  {#if tab === 'connections'}
    <Connections embedded {approval} onchange={loadCounts} />
  {:else}
    <Interpreters embedded {approval} onchange={loadCounts} />
  {/if}
{/key}

<style>
  .approval-kinds {
    display: flex;
    margin-bottom: 0.9rem;
  }
</style>
