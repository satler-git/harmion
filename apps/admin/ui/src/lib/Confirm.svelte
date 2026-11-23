<script lang="ts">
  import type { Snippet } from 'svelte';
  let {
    title,
    children,
    confirmLabel = '確認して実行',
    busy = false,
    danger = false,
    disabled = false,
    onsubmit,
    oncancel,
  }: {
    title: string;
    children: Snippet;
    confirmLabel?: string;
    busy?: boolean;
    danger?: boolean;
    disabled?: boolean;
    onsubmit: () => void;
    oncancel: () => void;
  } = $props();
</script>

<div class="confirmation" class:danger role="group" aria-label={title}>
  <h3>{title}</h3>
  {@render children()}
  <div class="actions">
    <button class={danger ? 'danger' : 'primary'} disabled={busy || disabled} onclick={onsubmit}>
      {busy ? '処理中…' : confirmLabel}
    </button><button disabled={busy} onclick={oncancel}>キャンセル</button>
  </div>
</div>
