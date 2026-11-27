<script lang="ts">
  import { api, errorMessage, fieldValue, writes } from './api.svelte';
  import type { Candidate, Edit, MergePlan, MergeRequest } from './types';
  import Confirm from './Confirm.svelte';
  let {
    content,
    token,
    candidates,
    save,
  }: {
    content: string;
    token: string;
    candidates: Candidate[];
    save: (edit: Edit) => Promise<void>;
  } = $props();
  let left = $state(0),
    right = $state(1),
    plan = $state<MergePlan | null>(null),
    text = $state(''),
    baseText = $state('');
  let busy = $state(false),
    error = $state(''),
    editable = $state(false),
    confirming = $state(false);
  let pairRevision = 0;
  function reset() {
    pairRevision++;
    plan = null;
    editable = false;
    baseText = '';
    error = '';
    confirming = false;
  }
  function input(base: string | null = null): MergeRequest {
    return {
      content,
      token,
      left: candidates[left].versions[0],
      right: candidates[right].versions[0],
      base,
    };
  }
  async function propose(base: string | null = null) {
    error = '';
    if (left === right) {
      error = '異なる2つの候補を選択してください。';
      return;
    }
    const revision = ++pairRevision,
      request = input(base);
    busy = true;
    try {
      const result = await api<MergePlan>('/merge', request);
      if (revision !== pairRevision) return;
      plan = result;
      if (result.status === 'proposed') {
        text = result.text;
        editable = true;
      }
    } catch (e) {
      if (revision === pairRevision) error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
  async function showBase() {
    if (plan?.status !== 'proposed') return;
    const revision = pairRevision;
    try {
      const result = await api<{ text: string }>('/merge-base', input(plan.base_version));
      if (revision === pairRevision) baseText = result.text;
    } catch (e) {
      error = errorMessage(e);
    }
  }
  async function submit() {
    if (!confirming) return;
    confirming = false;
    await save({
      type: 'body',
      text,
      versions: [...candidates[left].versions, ...candidates[right].versions],
    });
  }
</script>

<div class="merge">
  <div class="merge-head">
    <div>
      <h3>本文を編集・マージ</h3>
    </div>
  </div>
  <div class="actions">
    <label>
      左の候補
      <select bind:value={left} onchange={reset}>
        {#each candidates as _, i}<option value={i}>候補 {i + 1}</option>{/each}
      </select>
    </label>
    <label>
      右の候補
      <select bind:value={right} onchange={reset}>
        {#each candidates as _, i}<option value={i}>候補 {i + 1}</option>{/each}
      </select>
    </label>
    <button disabled={busy || writes.busy} onclick={() => propose()}>
      {busy ? '生成中…' : '三者マージ案を生成'}
    </button>
    <button
      disabled={busy}
      onclick={() => {
        reset();
        if (left === right) {
          error = '異なる2つの候補を選択してください。';
          return;
        }
        text = String(fieldValue(candidates[left].value));
        editable = true;
      }}
    >
      左の候補を手動で編集
    </button>
  </div>
  {#if error}<p role="alert" class="error">{error}</p>{/if}
  {#if plan?.status === 'choose_base'}<p>
      共通祖先が複数あります。使用する本文版を選択してください。
    </p>
    {#each plan.versions as version}<button
        class="mono"
        disabled={busy}
        onclick={() => propose(version)}
      >
        {version}
      </button>{/each}
  {:else if plan?.status === 'no_base'}<p>
      共通の本文版を取得できません。候補を比較して手動で編集してください。
    </p>
  {:else if plan?.status === 'proposed'}
    <p class:warning={plan.has_conflicts} class:success={!plan.has_conflicts}>
      {plan.has_conflicts
        ? '変更が重なっています。競合マーカーを含む箇所を編集して確認してください。'
        : '変更を組み合わせました。保存する前に本文を確認してください。'}
    </p>
    <button onclick={showBase}>共通祖先の本文を表示</button>
    {#if baseText}<pre>{baseText}</pre>{/if}
  {/if}
  {#if editable}
    <label for="merge-body">保存する本文</label><textarea
      id="merge-body"
      class="editor"
      bind:value={text}
      spellcheck="false"></textarea>
    <p class="muted small">この操作が置き換えるのは選択した候補です。新たに届いた版は残ります。</p>
    <button class="primary" disabled={writes.busy} onclick={() => (confirming = true)}>
      確認した本文を保存
    </button>
    {#if confirming}
      <Confirm
        title="本文を保存して競合を解決"
        busy={writes.busy}
        confirmLabel="本文を保存"
        onsubmit={submit}
        oncancel={() => (confirming = false)}
      >
        <p>候補 {left + 1} と候補 {right + 1} を、次の本文で置き換えます。</p>
        <pre class="body-text merge-preview">{text}</pre>
      </Confirm>
    {/if}
  {/if}
</div>

<style>
  .merge-head h3 {
    margin: 0 0 0.2rem;
  }
  .merge-preview {
    max-height: 14rem;
    margin: 0.5rem 0;
  }
</style>
