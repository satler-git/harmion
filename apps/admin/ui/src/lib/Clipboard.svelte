<script lang="ts">
  import { errorMessage } from './api.svelte';
  let { text, label = 'コピー' }: { text: string; label?: string } = $props();
  let feedback = $state('');
  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      feedback = 'コピーしました';
    } catch (error) {
      feedback = errorMessage(error) + ' テキストを選択してコピーしてください。';
    }
  }
</script>

<button onclick={copy}>{label}</button>
{#if feedback}<span class="muted small" role="status">{feedback}</span>{/if}
