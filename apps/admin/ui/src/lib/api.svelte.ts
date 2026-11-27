import type { Capabilities, Edit, Save } from './types';
export class ApiError extends Error {
  constructor(public status: number) {
    const messages: Record<number, string> = {
      400: '入力の形式、対象のノード、または要求の有効性を確認してください。',
      401: '認証が無効か、所有者セッションが終了しています。管理画面を開き直してください。',
      403: 'この操作に必要な権限がありません。',
      404: '対象が見つかりません。',
      409: '状態が変化したか、出力先が既に存在します。最新の状態を確認してください。',
      410: '要求の期限が切れました。もう一度読み込んでください。',
      413: 'この要求またはContentは一括取得のサイズ上限を超えています。',
      429: '処理中の要求が多いため、少し待って再実行してください。',
      503: 'Provider が未接続か、この機能を提供していません。',
      502: 'ノードとの接続または処理に失敗しました。ノードの起動状態を確認してください。',
      504: '応答を待機できませんでした。保存操作は同じ要求で再試行してください。',
    };
    super(messages[status] ?? `処理に失敗しました（HTTP ${status}）。`);
  }
}
export const session = $state({
  token: '',
  expiresAt: 0,
  ended: false,
  mode: 'native' as 'native' | 'worker',
});
export const workerAdmin = $state({ capabilities: null as Capabilities | null });
const MARKER = (window as unknown as { __HARMION_ADMIN_MODE__?: string }).__HARMION_ADMIN_MODE__;
// A Worker requires a bearer (401) even for capabilities; the native admin
// server explicitly answers 404 for that Worker-only path. This distinguishes
// the two modes without an unauthenticated endpoint or an HTML marker.
export async function detectMode(): Promise<'native' | 'worker'> {
  if (MARKER === 'worker') return 'worker';
  try {
    await api('/capabilities', {});
    return 'worker';
  } catch (error) {
    if (error instanceof ApiError && error.status === 401) return 'worker';
  }
  return 'native';
}

export async function api<T>(path: string, body: unknown): Promise<T> {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  if (session.token) headers.Authorization = 'Bearer ' + session.token;
  let response: Response;
  try {
    response = await fetch(path, {
      method: 'POST',
      headers,
      credentials: 'same-origin',
      body: JSON.stringify(body),
    });
  } catch {
    throw new Error('管理画面に接続できません。ネットワークと認証情報を確認してください。');
  }
  if (response.status === 401 && session.token) {
    session.token = '';
    session.ended = true;
  }
  if (!response.ok) throw new ApiError(response.status);
  return response.json();
}
export async function connect() {
  const launch = location.hash.slice(1);
  history.replaceState(null, '', location.pathname + location.search);
  if (!/^[a-f0-9]{64}$/.test(launch)) return false;
  const data = await api<{ session: string; expires_in: number }>('/session', launch);
  session.token = data.session;
  session.expiresAt = Date.now() + data.expires_in * 1000;
  return true;
}
export async function endSession() {
  if (session.mode === 'worker') {
    sessionStorage.removeItem('harmion-admin-token');
    session.token = '';
    session.ended = true;
    return;
  }
  try {
    await api('/session/end', {});
  } finally {
    session.token = '';
    session.ended = true;
  }
}

// Worker mode: the host injects window.__HARMION_ADMIN_MODE__ = "worker" and
// requires a deployment-issued bearer in addition to Cloudflare Access.
export async function loadCapabilities(): Promise<Capabilities | null> {
  try {
    const value = await api<Capabilities>('/capabilities', {});
    workerAdmin.capabilities = value;
    return value;
  } catch (error) {
    if (error instanceof ApiError && (error.status === 404 || error.status === 400)) {
      workerAdmin.capabilities = null;
      return null;
    }
    throw error;
  }
}
export async function signInWorker(token: string) {
  session.token = token.trim();
  session.mode = 'worker';
  try {
    const value = await loadCapabilities();
    if (!value) throw new Error('このWorkerは管理APIを提供していません。');
    sessionStorage.setItem('harmion-admin-token', session.token);
  } catch (error) {
    session.token = '';
    throw error;
  }
}
export function restoreWorkerSession(): string {
  return sessionStorage.getItem('harmion-admin-token') ?? '';
}
export function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : '処理に失敗しました。';
}
export function short(id: string) {
  return id.slice(0, 10) + '…' + id.slice(-6);
}
export function fieldName(field: import('./types').Field) {
  return field.type === 'body'
    ? '本文'
    : field.type === 'kind'
      ? 'Kind · ' + field.name
      : field.namespace + ' / ' + field.key;
}
export function fieldValue(value: import('./types').FieldValue) {
  if (value.type === 'absent') return '未設定';
  if (value.type === 'kind') return value.value ? '所属する' : '所属しない';
  return value.value;
}
export function count(input: string) {
  return BigInt(input).toLocaleString('ja-JP');
}

// Keep an ambiguous write and its exact request ID across component/navigation changes.
export const writes = $state<{ pending: Save | null; busy: boolean; error: string }>({
  pending: null,
  busy: false,
  error: '',
});
export async function sendSave(content: string, token: string, edit: Edit) {
  if (writes.busy) throw new Error('保存中です。完了までお待ちください。');
  if (
    writes.pending &&
    JSON.stringify({ content, token, edit }) !==
      JSON.stringify({
        content: writes.pending.content,
        token: writes.pending.token,
        edit: writes.pending.edit,
      })
  )
    throw new Error(
      '前の保存結果を確認できていません。「同じ要求で再試行」で結果を確認してください。',
    );
  writes.pending ??= {
    content,
    token,
    edit,
    request_id: Array.from(crypto.getRandomValues(new Uint8Array(32)), (b) =>
      b.toString(16).padStart(2, '0'),
    ).join(''),
  };
  writes.busy = true;
  writes.error = '';
  try {
    const result = await api<{ saved: boolean; update_id: string }>('/resolve', writes.pending);
    writes.pending = null;
    return result;
  } catch (error) {
    if (error instanceof ApiError && [400, 401, 403, 404, 409, 413].includes(error.status))
      writes.pending = null;
    writes.error = errorMessage(error);
    throw error;
  } finally {
    writes.busy = false;
  }
}
export async function retrySave() {
  if (!writes.pending) return;
  const { content, token, edit } = writes.pending;
  return sendSave(content, token, edit);
}
