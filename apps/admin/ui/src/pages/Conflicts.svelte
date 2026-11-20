<script lang="ts">
  import { api, errorMessage, fieldName, fieldValue, sendSave, writes } from '../lib/api.svelte';
  import type { Inspection, Edit, Selection } from '../lib/types';
  import Merge from '../lib/Merge.svelte';
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
  let generation = 0;
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
    try {
      const result = await api<Inspection>('/inspect', { content: target });
      if (ticket !== generation) return;
      opened = target;
      id = target;
      data = result;
      choices = {};
      restoreChoices = {};
      restoreText = String(
        fieldValue(result.groups.find((g) => g.field.type === 'body')!.candidates[0].value),
      );
      result.groups.forEach((group, i) => {
        if (group.field.type !== 'body')
          restoreChoices[i] = group.candidates.length === 1 ? '0' : '';
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
  async function select() {
    if (!data) return;
    const fields: Selection[] = data.groups.flatMap((group, i) => {
      const selected = choices[i];
      if (selected === undefined || selected === '') return [];
      return [
        {
          field: group.field,
          version: group.candidates[Number(selected)].versions[0],
          replaces: group.candidates.flatMap((c) => c.versions),
        },
      ];
    });
    if (!fields.length) {
      error = '保存する候補を選んでください。';
      return;
    }
    if (confirm('選択した値で競合を解決しますか？')) await save({ type: 'select', fields });
  }
  async function restore() {
    if (!data) return;
    const fields = data.groups.flatMap((g, i) =>
      g.field.type === 'body' || restoreChoices[i] === 'omit'
        ? []
        : [
            {
              field: g.field,
              version: g.candidates[Number(restoreChoices[i])]?.versions[0],
            },
          ],
    );
    if (fields.some((f) => !f.version)) {
      error = '競合している属性・kind の値を選択してください。';
      return;
    }
    if (confirm('表示中の本文・属性・kind で Content を復元しますか？'))
      await save({ type: 'restore', text: restoreText, fields });
  }
</script>

<div class="page-title">
  <div>
    <p class="eyebrow">CONFLICT RESOLVER</p>
    <h1>競合解決</h1>
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
      Content ID<input
        bind:value={id}
        required
        pattern={'[a-fA-F0-9]{64}'}
        placeholder="コンテンツ画面から選択するか、ID を貼り付け"
      />
    </label>
    <button disabled={busy || writes.busy}>{busy ? '取得中…' : '候補を取得'}</button>
  </form>
  {#if error}<p class="error" role="alert">{error}</p>{/if}{#if message}<p
      class="success"
      role="status"
    >
      {message}
    </p>{/if}
  {#if data}
    <p class="mono muted small">{opened}</p>
    {#if data.deleted}<p class="warning">削除済みの Content です。復元する値を確認してください。</p>
    {:else if data.deletion_conflict}<p class="warning">削除と編集が競合しています。</p>{/if}
    {#key data.token}
      {#each data.groups as group, i}
        <section class="field">
          <div class="page-title">
            <h2>{fieldName(group.field)}</h2>
            {#if group.candidates.length > 1}<span class="badge warn">
                {group.candidates.length} 候補
              </span>{/if}
          </div>
          <div class="candidates">
            {#each group.candidates as candidate, j}
              <article>
                <h3>候補 {j + 1}</h3>
                <pre class="body-text">{fieldValue(candidate.value)}</pre>
                <details>
                  <summary>版 ID</summary>
                  <pre class="muted small">{candidate.versions.join('\n')}</pre>
                </details>
              </article>
            {/each}
          </div>
          {#if group.candidates.length > 1 && !data.deleted}
            <label>
              保存する値<select bind:value={choices[i]}>
                <option value="">未解決のままにする</option>{#each group.candidates as _, j}<option
                    value={String(j)}
                  >
                    候補 {j + 1} を採用
                  </option>{/each}
              </select>
            </label>
            {#if group.field.type === 'body'}<Merge
                content={opened}
                token={data.token}
                candidates={group.candidates}
                {save}
              />{/if}
          {/if}
        </section>
      {/each}
      {#if !data.deleted && data.groups.some((g) => g.candidates.length > 1)}<button
          class="primary"
          disabled={writes.busy}
          onclick={select}
        >
          選択した値を保存
        </button>{/if}
      {#if data.deletion_conflict}<div class="actions field">
          <button disabled={writes.busy} onclick={() => save({ type: 'keep' })}>
            Content を残す
          </button><button
            class="danger"
            disabled={writes.busy}
            onclick={() => {
              if (confirm('確認した編集と削除を対象に削除を確定しますか？'))
                void save({ type: 'delete' });
            }}
          >
            削除を確定
          </button>
        </div>{/if}
      {#if data.deleted}
        <section class="field">
          <h2>Content の復元</h2>
          <label for="restore-body">復元する本文</label><textarea
            id="restore-body"
            class="editor"
            bind:value={restoreText}></textarea>
          {#each data.groups as group, i}{#if group.field.type !== 'body'}<label>
                {fieldName(group.field)}<select bind:value={restoreChoices[i]}>
                  <option value="">値を選択</option>{#each group.candidates as _, j}<option
                      value={String(j)}
                    >
                      候補 {j + 1}
                    </option>{/each}<option value="omit">復元時に省略する</option>
                </select>
              </label>{/if}{/each}
          <button class="primary" disabled={writes.busy} onclick={restore}>確認して復元</button>
        </section>
      {/if}
    {/key}
  {:else if !busy}<div class="empty">
      <h2>解決する Content を選択</h2>
      <p>候補の本文・属性・kind を比較できます。削除済みの ID も指定できます。</p>
    </div>{/if}
</section>
