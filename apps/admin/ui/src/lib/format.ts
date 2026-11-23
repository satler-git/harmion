import type {
  ApprovalStatus,
  ConnectionStatus,
  Field,
  FieldValue,
  Filter,
  Grants,
  InterpreterStatus,
  Members,
  ProviderManifest,
} from './types';

const dateTimeFormat = new Intl.DateTimeFormat('ja-JP', {
  dateStyle: 'medium',
  timeStyle: 'short',
});

export function shortId(value: string | null | undefined, head = 10, tail = 6): string {
  if (!value) return '—';
  if (value.length <= head + tail + 1) return value;
  return `${value.slice(0, head)}…${value.slice(-tail)}`;
}

/** API timestamps are decimal microseconds since the Unix epoch. */
export function microsDate(value: string | null | undefined): Date | null {
  if (!value) return null;
  const micros = Number(value);
  if (!Number.isFinite(micros)) return null;
  return new Date(micros / 1000);
}

export function formatDateTime(value: string | null | undefined): string {
  const date = microsDate(value);
  return date ? dateTimeFormat.format(date) : '—';
}

export function relativeTime(value: string | null | undefined, now = Date.now()): string {
  const date = microsDate(value);
  if (!date) return '—';
  const seconds = Math.round((date.getTime() - now) / 1000);
  const future = seconds >= 0;
  const amount = Math.abs(seconds);
  let text: string;
  if (amount < 60) text = `${amount} 秒`;
  else if (amount < 3600) text = `${Math.floor(amount / 60)} 分`;
  else if (amount < 86400) text = `${Math.floor(amount / 3600)} 時間`;
  else text = `${Math.floor(amount / 86400)} 日`;
  return future ? `あと ${text}` : `${text}前`;
}

export function remainingTime(value: string | null | undefined, now = Date.now()): string {
  const date = microsDate(value);
  if (!date) return '—';
  const seconds = Math.floor((date.getTime() - now) / 1000);
  if (seconds <= 0) return '期限切れ';
  if (seconds < 60) return `残り ${seconds} 秒`;
  if (seconds < 3600) return `残り ${Math.floor(seconds / 60)} 分`;
  return `残り ${Math.floor(seconds / 3600)} 時間 ${Math.floor((seconds % 3600) / 60)} 分`;
}

export function filterLabel(filter: Filter | null | undefined): string {
  if (!filter) return 'なし';
  if (filter.type === 'all') return 'すべての Content';
  return filter.kinds.length ? `Kind: ${filter.kinds.join('、')}` : 'Kind の指定なし';
}

export interface GrantRow {
  label: string;
  value: string;
  active: boolean;
}

export function grantRows(grants: Grants | null | undefined): GrantRow[] {
  if (!grants) return [];
  const rows: GrantRow[] = [
    { label: '読み取り', value: filterLabel(grants.read), active: grants.read !== null },
    { label: '書き込み', value: filterLabel(grants.write), active: grants.write !== null },
    { label: 'Interpreter 呼び出し', value: grants.call ? '可' : '不可', active: grants.call },
    {
      label: 'Interpreter 提供',
      value: grants.provide
        ? grants.provide_names?.length
          ? `可 · ${grants.provide_names.join('、')}`
          : '可 · すべての Provider 名'
        : '不可',
      active: grants.provide,
    },
  ];
  if (grants.manage_apps) rows.push({ label: 'アプリ管理', value: '可', active: true });
  if (grants.manage_nodes) rows.push({ label: 'ノード管理', value: '可', active: true });
  return rows;
}

export function providerHint(grants: Grants | null | undefined): string {
  if (!grants?.provide) return 'この接続は Interpreter を提供しません。';
  if (grants.provide_names?.length)
    return `次の名前だけを提供できます: ${grants.provide_names.join('、')}`;
  return 'すべての Provider 名を提供できます。';
}

export function defaultKinds(provider: ProviderManifest): string {
  const kinds = provider.default_query?.kinds ?? [];
  return kinds.length ? kinds.join('、') : '指定なし';
}

export function prettyJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2) ?? 'null';
  } catch {
    return String(value);
  }
}

export function connectionStatusLabel(status: ConnectionStatus): string {
  switch (status) {
    case 'pending':
      return '承認待ち';
    case 'approved':
      return '承認済み・完了待ち';
    case 'denied':
      return '拒否';
    case 'completed':
      return '接続完了';
    case 'expired':
      return '期限切れ';
    default:
      return status;
  }
}

export function connectionStatusTone(status: ConnectionStatus): string {
  switch (status) {
    case 'pending':
      return 'badge-warn';
    case 'approved':
      return 'badge-info';
    case 'completed':
      return 'badge-ok';
    case 'denied':
    case 'expired':
      return 'badge-muted';
    default:
      return '';
  }
}

export function interpreterStatusLabel(status: InterpreterStatus): string {
  switch (status) {
    case 'approved':
      return '承認済み';
    case 'pending':
      return '承認待ち';
    case 'superseded':
      return '置換済み';
    default:
      return status;
  }
}

export function interpreterStatusTone(status: InterpreterStatus): string {
  switch (status) {
    case 'approved':
      return 'badge-ok';
    case 'pending':
      return 'badge-warn';
    case 'superseded':
      return 'badge-muted';
    default:
      return '';
  }
}

export function membershipStateLabel(state: Members['members'][number]['state']): string {
  switch (state) {
    case 'active':
      return '有効';
    case 'retired':
      return '退役';
    case 'conflict':
      return '競合';
    case 'unconfirmed':
      return '未確認';
    default:
      return state;
  }
}

export function membershipStateTone(state: Members['members'][number]['state']): string {
  switch (state) {
    case 'active':
      return 'badge-ok';
    case 'retired':
    case 'unconfirmed':
      return 'badge-muted';
    case 'conflict':
      return 'badge-warn';
    default:
      return '';
  }
}

export function pinProvenanceLabel(provenance: string): string {
  switch (provenance) {
    case 'owner_registration':
      return '所有者の登録';
    case 'owner_recovery':
      return '所有者の回復';
    case 'accepted_admission':
      return '参加証明の受理';
    case 'legacy_import':
      return '旧形式からの移行';
    default:
      return provenance;
  }
}

export function transportLabel(transport: string): string {
  switch (transport) {
    case 'iroh':
      return 'iroh';
    case 'https':
      return 'HTTPS（証明書固定）';
    case 'signed_https':
      return '署名付き HTTPS';
    default:
      return transport;
  }
}

/** Human-readable target of a binding, never the raw certificate itself. */
export function bindingTarget(binding: {
  transport: string;
  endpoint?: string;
  origin?: string;
  relay_url?: string | null;
  endpoint_id?: string;
}): string {
  if (binding.origin) return binding.origin;
  if (binding.endpoint) return binding.endpoint;
  if (binding.relay_url) return binding.relay_url;
  if (binding.endpoint_id) return shortId(binding.endpoint_id);
  return '—';
}

export function fieldLabel(field: Field): string {
  return field.type === 'body'
    ? '本文'
    : field.type === 'kind'
      ? `Kind · ${field.name}`
      : `${field.namespace} / ${field.key}`;
}

export function fieldValueText(value: FieldValue): string {
  if (value.type === 'absent') return '未設定';
  if (value.type === 'kind') return value.value ? '所属する' : '所属しない';
  return value.value;
}

export function approvalModeLabel(status: ApprovalStatus): string {
  switch (status.mode) {
    case 'terminal':
      return status.terminal_available
        ? '起動したターミナルで y を押して承認'
        : '起動端末が TTY でないため、R2 操作は拒否されます';
    case 'stdio':
      return '監督下の標準入力から承認（自動化向け）';
    case 'session':
      return 'ブラウザの確認だけで承認（端末確認なし）';
    case 'deny':
      return 'R2 操作は拒否する設定です';
    default:
      return status.mode;
  }
}

export function approvalTone(status: ApprovalStatus): string {
  if (status.mode === 'deny') return 'badge-danger';
  if (status.mode === 'session') return 'badge-warn';
  if (status.mode === 'terminal' && !status.terminal_available) return 'badge-danger';
  return 'badge-info';
}
