export type Page = 'overview' | 'contents' | 'conflicts' | 'peers' | 'apps' | 'export';
export type Field =
  | { type: 'body' }
  | { type: 'attribute'; namespace: string; key: string }
  | { type: 'kind'; name: string };
export type FieldValue =
  | { type: 'body' | 'attribute'; value: string }
  | { type: 'kind'; value: boolean }
  | { type: 'absent' };
export interface Candidate {
  value: FieldValue;
  versions: string[];
}
export interface Group {
  field: Field;
  candidates: Candidate[];
}
export interface Inspection {
  token: string;
  deleted: boolean;
  deletion_conflict: boolean;
  groups: Group[];
}
export interface ContentItem {
  id: string;
  preview: string;
  body: string;
  kinds: string[];
  attributes: string;
  conflicted: boolean;
}
export interface ContentPage {
  items: ContentItem[];
  next: string | null;
}
export interface Selection {
  field: Field;
  version: string;
  replaces: string[];
}
export type Edit =
  | { type: 'select'; fields: Selection[] }
  | { type: 'body'; text: string; versions: string[] }
  | { type: 'delete' | 'keep' }
  | { type: 'restore'; text: string; fields: { field: Field; version: string }[] };
export interface Save {
  content: string;
  token: string;
  request_id: string;
  edit: Edit;
}
export type MergePlan =
  | { status: 'proposed'; base_version: string; text: string; has_conflicts: boolean }
  | { status: 'choose_base'; versions: string[] }
  | { status: 'no_base' };
export interface MergeRequest {
  content: string;
  token: string;
  left: string;
  right: string;
  base: string | null;
}
export interface Registration {
  version: number;
  cluster_id: string;
  node_id: string;
  signing_key?: string;
  cluster_root?: string;
  binding:
    | { transport: 'iroh'; endpoint_id: string; relay_url: string | null }
    | { transport: 'https'; certificate: string; endpoint: string };
}
export type Filter = { type: 'all' } | { type: 'kinds'; kinds: string[] };
export interface Grants {
  read: Filter | null;
  write: Filter | null;
  manage_apps: boolean;
  manage_nodes: boolean;
}
export interface Members {
  members: { node_id: string; state: 'active' | 'retired' | 'conflict' | 'unconfirmed' }[];
  pending_retirements: { certificate_id: string; issuer: string; target: string }[];
  confirmed_certificates: number;
}
export interface Overview {
  identity: Registration;
  permissions: Grants;
  capabilities: { oauth: boolean; oauth_origin: string | null; [key: string]: unknown };
  metrics: {
    contents: string;
    visible_contents: string;
    accepted_updates: string;
    record_bytes: string;
    active_apps: string;
  };
  members: Members;
  peer_transfer: string;
}
export interface PeerPreview {
  ticket: string;
  registration: Registration;
  action: 'approve' | 'join';
}
export type AppPreview =
  | { type: 'registration'; name: string; redirect_uri: string }
  | {
      type: 'authorization';
      request: string;
      preview: {
        client_id: string;
        name: string;
        redirect_uri: string;
        grants: Grants;
        expires_at: string;
      };
    };
export interface SyncResult {
  uploaded: string;
  downloaded: string;
  local_high: string;
  remote_high: string;
}
