<script lang="ts">
  import { api, errorMessage, sendSave, writes } from '../lib/api.svelte';
  import type { Field, Inspection, Edit, Selection } from '../lib/types';
  import Merge from '../lib/Merge.svelte';
  import Confirm from '../lib/Confirm.svelte';
  import { fieldLabel, fieldValueText } from '../lib/format';

  let { requested }: { requested: string } = $props();
  let id = $state(''),
    opened = $state(''),
    data = $state<Inspection | null>(null),
    error = $state(''),
    message = $state(''),
    busy = $state(false);
  let choices = $state<Record<number, string>>({}),
    restoreText = $state(''),
    restoreChoices = $state<Record<number, string>>({});
  type Ask =
    | { kind: 'select'; fields: Selection[] }
    | { kind: 'delete' }
    | { kind: 'restore'; fields: { field: Field; version: string }[]; text: string };
  let ask = $state<Ask | null>(null);
  let generation = 0;

  const candidateCount = $derived(
    data ? data.groups.reduce((sum, group) => sum + group.candidates.length, 0) : 0,
  );
  const unresolved = $derived(
    data ? data.groups.filter((group) => group.candidates.length > 1).length : 0,
  );

  async function load(target = id) {
    target = target.trim();
    if (!/^[a-fA-F0-9]{64}$/.test(target)) {
      error = '64桁の Content ID を入力してください。';
      return;
    }
    const ticket = ++generation;
    busy = true;
    error = '';
    message = '';
    ask = null;
    try {
      const result = await api<Inspection>('/inspect', { content: target });
      if (ticket !== generation) return;
      opened = target;
      id = target;
      data = result;
      choices = {};
      restoreChoices = {};
      const body = result.groups.find((group) => group.field.type === 'body');
      const firstBody = body?.candidates[0];
      restoreText = firstBody ? String(fieldValueText(firstBody.value)) : '';
      result.groups.forEach((group, index) => {
        if (group.field.type !== 'body')
          restoreChoices[index] = group.candidates.length === 1 ? '0' : '';
      });
    } catch (e) {
      if (ticket === generation) error = errorMessage(e);
    } finally {
      if (ticket === generation) busy = false;
    }
  }
  let lastRequested = '';
  $effect(() => {
    if (requested && requested !== lastRequested) {
      lastRequested = requested;
      void load(requested);
    }
  });
  async function save(edit: Edit) {
    if (!data) return;
    error = '';
    try {
      await sendSave(opened, data.token, edit);
      await load(opened);
      message = '保存しました。現在の候補を表示しています。';
    } catch (e) {
      error = errorMessage(e);
    }
  }
  function requestSelect() {
    if (!data) return;
    const fields: Selection[] = data.groups.flatMap((group, index) => {
      const selected = choices[index];
      if (selected === undefined || selected === '') return [];
      const version = group.candidates[Number(selected)]?.versions[0];
      if (!version) return [];
      return [
        {
          field: group.field,
          version,
          replaces: group.candidates.flatMap((candidate) => candidate.versions),
        },
      ];
    });
    if (!fields.length) {
      error = '保存する候補を選んでください。';
      return;
    }
    ask = { kind: 'select', fields };
  }
  function requestRestore() {
    if (!data) return;
    const fields = data.groups.flatMap((group, index) => {
      if (group.field.type === 'body' || restoreChoices[index] === 'omit') return [];
      const version = group.candidates[Number(restoreChoices[index])]?.versions[0];
      return version ? [{ field: group.field, version }] : [];
    });
    const required = data.groups.filter(
      (group, index) => group.field.type !== 'body' && restoreChoices[index] !== 'omit',
    ).length;
    if (fields.length !== required) {
      error = '競合している属性・Kind の値を選択してください。';
      return;
    }
    ask = { kind: 'restore', fields, text: restoreText };
  }
  async function runAsk() {
    const action = ask;
    if (!action) return;
    ask = null;
    if (action.kind === 'select') await save({ type: 'select', fields: action.fields });
    else if (action.kind === 'delete') await save({ type: 'delete' });
    else await save({ type: 'restore', text: action.text, fields: action.fields });
  }
</script>

<div class="page-title">
  <div>
    <p class="eyebrow">CONFLICT RESOLVER</p>
    <h1>競合解決</h1>
    <p class="muted">
      同じフィールドに複数の値があるとき、採用する値・本文・削除の扱いを決めます。編集トークンは
      Content ごとに固定されているため、解決するたびに候補を再取得します。
    </p>
  </div>
</div>

<section class="panel">
  <form
    class="actions"
    onsubmit={(event) => {
      event.preventDefault();
      void load();
    }}
  >
    <label class="grow">
      Content ID
      <input
        bind:value={id}
        required
        pattern={'[a-fA-F0-9]{64}'}
        placeholder="コンテンツ画面から選択するか、64桁の ID を貼り付け"
      />
    </label>
    <button disabled={busy || writes.busy}>{busy ? '取得中…' : '候補を取得'}</button>
  </form>

  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if message}<p class="success" role="status">{message}</p>{/if}

  {#if data}
    <div class="resolver-head">
      <div>
        <p class="mono muted small">{opened}</p>
        <div class="title-line">
          <span class="badge">{data.groups.length} フィールド</span>
          <span class="badge">{candidateCount} 候補</span>
          {#if unresolved}<span class="badge badge-warn">{unresolved} 未解決</span>{/if}
          {#if data.deleted}<span class="badge badge-muted">削除済み</span>{/if}
          {#if data.deletion_conflict}<span class="badge badge-warn">削除と編集が競合</span>{/if}
        </div>
      </div>
      <button class="quiet" disabled={busy || writes.busy} onclick={() => load(opened)}>
        候補を再取得
      </button>
    </div>

    {#if data.deleted}
      <p class="warning">削除済みの Content です。復元する本文と属性を確認してください。</p>
    {:else if data.deletion_conflict}
      <p class="warning">削除と編集が競合しています。残すか、削除を確定するか選んでください。</p>
    {/if}

    {#key data.token}
      {#each data.groups as group, index}
        <section class="field">
          <div class="field-head">
            <div>
              <h2>{fieldLabel(group.field)}</h2>
              <p class="muted small">
                {group.field.type === 'body'
                  ? '本文の候補を比較し、必要ならマージします。'
                  : group.field.type === 'kind'
                    ? 'この Kind に所属するかどうかを選びます。'
                    : '属性の値を選びます。'}
              </p>
            </div>
            {#if group.candidates.length > 1}
              <span class="badge badge-warn">{group.candidates.length} 候補</span>
            {:else}
              <span class="badge badge-ok">確定済み</span>
            {/if}
          </div>
          <div class="candidates">
            {#each group.candidates as candidate, candidateIndex}
              <article>
                <div class="candidate-head">
                  <h3>候補 {candidateIndex + 1}</h3>
                  {#if group.candidates.length > 1 && !data.deleted}
                    <label class="check compact">
                      <input
                        type="radio"
                        name={`choice-${index}`}
                        value={String(candidateIndex)}
                        bind:group={choices[index]}
                      />
                      採用
                    </label>
                  {/if}
                </div>
                <pre class="body-text">{fieldValueText(candidate.value)}</pre>
                <details>
                  <summary>この候補の版 ID</summary>
                  <pre class="muted small">{candidate.versions.join('\n')}</pre>
                </details>
              </article>
            {/each}
          </div>
          {#if group.field.type === 'body' && group.candidates.length > 1 && !data.deleted}
            <Merge content={opened} token={data.token} candidates={group.candidates} {save} />
          {/if}
          {#if group.candidates.length > 1 && !data.deleted}
            <label>
              保存する値
              <select bind:value={choices[index]}>
                <option value="">未解決のままにする</option>
                {#each group.candidates as _, candidateIndex}
                  <option value={String(candidateIndex)}>候補 {candidateIndex + 1} を採用</option>
                {/each}
              </select>
            </label>
          {/if}
        </section>
      {/each}

      {#if !data.deleted && data.groups.some((group) => group.candidates.length > 1)}
        <div class="actions">
          <button class="primary" disabled={writes.busy} onclick={requestSelect}>
            選択した値を保存
          </button>
          {#if unresolved}
            <span class="muted small">未選択のフィールドは未解決のまま残ります。</span>
          {/if}
        </div>
      {/if}

      {#if data.deletion_conflict}
        <div class="actions field">
          <button disabled={writes.busy} onclick={() => save({ type: 'keep' })}>
            Content を残す
          </button>
          <button class="danger" disabled={writes.busy} onclick={() => (ask = { kind: 'delete' })}>
            削除を確定
          </button>
        </div>
      {/if}

      {#if data.deleted}
        <section class="field">
          <div class="field-head">
            <div>
              <h2>Content の復元</h2>
              <p class="muted small">
                本文と属性を確認して復元します。属性は必要なものだけ選び、不要なものは省略できます。
              </p>
            </div>
          </div>
          <label for="restore-body">復元する本文</label>
          <textarea id="restore-body" class="editor" bind:value={restoreText}></textarea>
          {#each data.groups as group, index}
            {#if group.field.type !== 'body'}
              <label>
                {fieldLabel(group.field)}
                <select bind:value={restoreChoices[index]}>
                  <option value="">値を選択</option>
                  {#each group.candidates as _, candidateIndex}
                    <option value={String(candidateIndex)}>候補 {candidateIndex + 1}</option>
                  {/each}
                  <option value="omit">復元時に省略する</option>
                </select>
              </label>
            {/if}
          {/each}
          <button class="primary" disabled={writes.busy} onclick={requestRestore}>
            確認して復元
          </button>
        </section>
      {/if}
    {/key}
  {:else if !busy}
    <div class="empty">
      <h2>解決する Content を選択</h2>
      <p>候補の本文・属性・Kind を比較できます。削除済みの ID も指定できます。</p>
    </div>
  {/if}
</section>

{#if ask}
  <Confirm
    title={ask.kind === 'select'
      ? '選択した値で競合を解決'
      : ask.kind === 'delete'
        ? '削除を確定'
        : 'Content を復元'}
    danger={ask.kind === 'delete'}
    busy={writes.busy}
    onsubmit={runAsk}
    oncancel={() => (ask = null)}
  >
    {#if ask.kind === 'select'}
      <p>
        {ask.fields.length} フィールドの値を保存します。選択しなかったフィールドは未解決のまま残ります。
      </p>
    {:else if ask.kind === 'delete'}
      <p>確認した編集と削除を対象に、削除を確定します。この操作は取り消せません。</p>
    {:else}
      <p>表示中の本文・属性・Kind で Content を復元します。</p>
      <p class="muted small">{ask.fields.length} 件の属性・Kind を復元に含めます。</p>
    {/if}
  </Confirm>
{/if}

<style>
  .resolver-head {
    display: flex;
    align-items: start;
    justify-content: space-between;
    gap: 1rem;
    margin: 1rem 0;
  }
  .resolver-head p {
    margin: 0 0 0.35rem;
  }
  .title-line {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
  }
  .field-head {
    display: flex;
    align-items: start;
    justify-content: space-between;
    gap: 1rem;
  }
  .field-head h2 {
    margin-bottom: 0.2rem;
  }
  .field-head p {
    margin: 0;
  }
  .candidate-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }
  .candidate-head h3 {
    margin: 0;
    flex: 1;
  }
  .check.compact {
    margin: 0;
    font-size: 0.8rem;
    font-weight: 500;
    white-space: nowrap;
    flex-shrink: 0;
  }
  .candidates article .body-text {
    margin: 0.6rem 0 0;
  }
  .candidates article details {
    margin-top: 0.5rem;
  }
  .candidates article summary {
    color: var(--accent);
    cursor: pointer;
    font-size: 0.78rem;
  }
  @media (max-width: 730px) {
    .resolver-head,
    .field-head {
      flex-direction: column;
    }
  }
</style>
