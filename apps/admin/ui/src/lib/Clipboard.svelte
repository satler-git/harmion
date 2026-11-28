<script lang="ts">
  import { errorMessage } from './api.svelte';
  import Icon from './Icon.svelte';
  let {
    text,
    label = 'コピー',
    iconOnly = false,
  }: { text: string; label?: string; iconOnly?: boolean } = $props();
  let feedback = $state('');
  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      feedback = 'コピーしました';
      setTimeout(() => (feedback = ''), 1600);
    } catch (error) {
      feedback = errorMessage(error) + ' テキストを選択してコピーしてください。';
    }
  }
</script>

<button
  class:icon-button={iconOnly}
  aria-label={iconOnly ? label : undefined}
  title={iconOnly ? label : undefined}
  onclick={copy}
>
  {#if iconOnly}<Icon name={feedback ? 'check' : 'copy'} size={14} />{:else}{label}{/if}
</button>
{#if feedback && !iconOnly}<span class="muted small" role="status">{feedback}</span>{/if}
