export interface Host {
  id: string;
  name: string;
  host: string;
  port: number;
  user: string;
  password?: string;
  keychain_id?: string;
  tags: string[];
}

export type TabType = 'home' | 'terminal' | 'sftp';

export interface AppTab {
  id: string;
  type: TabType;
  title: string;
  host?: Host;
}

export type KeychainItemType = 'Password' | 'PrivateKey';

export interface KeychainItem {
  id: string;
  name: string;
  item_type: KeychainItemType;
  value: String;
}

/** What a tunnel is doing right now — not what the rule asks for. */
export type TunnelState = 'active' | 'inactive' | 'error';

export interface PortForwardingRule {
  id: string;
  name: string;
  host_id: string;
  /** Resolved server-side; null when the rule points at a deleted host. */
  host_name: string | null;
  local_port: number;
  bind_address: string;
  destination_address: string;
  destination_port: number;
  /** Live: bound and the SSH session behind it is healthy. */
  active: boolean;
  /** A listener is bound — true even when `status` is 'error'. */
  running: boolean;
  /** Persisted intent: re-opened on restart. */
  enabled: boolean;
  status: TunnelState;
  listening_on: string | null;
  started_at: number | null;
  uptime_seconds: number | null;
  /** Connections piped through right now. */
  connections: number;
  total_connections: number;
  last_error: string | null;
}

/** Reply from the start/stop endpoints. */
export interface TunnelActionResponse {
  ok: boolean;
  error?: string;
  rule?: PortForwardingRule;
}

export interface FileNode {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  modified_time: number;
}

/** A reusable command fired into a terminal from the right sidebar. */
export interface Snippet {
  id: string;
  name: string;
  /** Command line(s). Multi-line scripts are sent line by line. */
  script: string;
  description?: string;
  tags: string[];
  /** Epoch millis, assigned by the server on create. */
  created_at: number;
}
