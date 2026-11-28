<script lang="ts">
  import QRCode from 'qrcode';
  import { errorMessage, loadRegistrationCode } from './api.svelte';
  import Clipboard from './Clipboard.svelte';
  import type { RegistrationCode } from './types';

  let { approved = false }: { approved?: boolean } = $props();
  let code = $state<RegistrationCode | null>(null);
  let qr = $state('');
  let qrError = $state('');
  let error = $state('');
  let loading = $state(false);

  async function load() {
    if (loading || code) return;
    loading = true;
    error = '';
    qrError = '';
    qr = '';
    try {
      const value = await loadRegistrationCode();
      code = value;
      try {
        qr = await QRCode.toDataURL(value.code, {
          width: 480,
          margin: 4,
          errorCorrectionLevel: 'L',
        });
      } catch {
        qrError = 'QR の容量を超えています。接続コードをコピーしてください。';
      }
    } catch (e) {
      error = errorMessage(e);
    } finally {
      loading = false;
    }
  }

  // Covers the initial unapproved state and a later switch from approved to
  // identity-mismatched (the panel replaces the collapsed details).
  $effect(() => {
    if (!approved && !code && !loading && !error) void load();
  });
</script>

{#if approved}
  <details
    class="panel registration"
    ontoggle={(event) => {
      if ((event.currentTarget as HTMLDetailsElement).open) void load();
    }}
  >
    <summary>登録コード</summary>
    {#if loading}
      <p class="muted small" role="status">読み込み中…</p>
    {:else if error}
      <p class="error" role="alert">{error}</p>
      <button type="button" onclick={load}>再読み込み</button>
    {:else if code}
      <div class="registration-body">
        {#if qr}
          <img
            class="qr"
            src={qr}
            alt="この Worker の登録コードを含む QR"
            width="240"
            height="240"
          />
        {/if}
        <div class="registration-code">
          <textarea class="code-input" aria-label="接続コード" readonly value={code.code}
          ></textarea>
          <Clipboard text={code.code} label="接続コードをコピー" />
        </div>
      </div>
      {#if qrError}<p class="warning">{qrError}</p>{/if}
    {/if}
  </details>
{:else}
  <section class="panel registration">
    <h2>この Worker を承認</h2>
    <p class="muted">ローカルのノード画面で読み取ります。</p>
    {#if loading}
      <p class="muted small" role="status">読み込み中…</p>
    {:else if error}
      <p class="error" role="alert">{error}</p>
      <button type="button" onclick={load}>再読み込み</button>
    {:else if code}
      <div class="registration-body">
        {#if qr}
          <img
            class="qr"
            src={qr}
            alt="この Worker の登録コードを含む QR"
            width="240"
            height="240"
          />
        {/if}
        <div class="registration-code">
          <textarea class="code-input" aria-label="接続コード" readonly value={code.code}
          ></textarea>
          <Clipboard text={code.code} label="接続コードをコピー" />
        </div>
      </div>
      {#if qrError}<p class="warning">{qrError}</p>{/if}
    {/if}
  </section>
{/if}

<style>
  .registration summary {
    color: var(--accent);
    cursor: pointer;
    font-weight: 600;
  }
  .registration-body {
    display: flex;
    gap: 1.2rem;
    align-items: start;
    margin-top: 1rem;
    flex-wrap: wrap;
  }
  .qr {
    background: #fff;
    border-radius: 8px;
    padding: 0.4rem;
    flex-shrink: 0;
  }
  .registration-code {
    flex: 1;
    min-width: 16rem;
  }
  .registration-code textarea {
    min-height: 9rem;
    margin-bottom: 0.5rem;
  }
</style>
