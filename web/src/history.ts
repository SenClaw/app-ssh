/**
 * Command history for the sidebar's clock panel.
 *
 * Kept client-side: the backend never sees interactive keystrokes — they go
 * straight down the terminal websocket — so the only place a typed command can
 * be observed is the xterm instance that produced it.
 */

const STORAGE_KEY = 'ssh-history';
const MAX_ENTRIES = 300;

export interface HistoryEntry {
  id: string;
  command: string;
  /** Host label the command was sent to, for context in the list. */
  host: string;
  /** Epoch millis. */
  at: number;
}

let entries: HistoryEntry[] = load();
const listeners = new Set<() => void>();

function load(): HistoryEntry[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    const parsed = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function persist() {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(entries));
  } catch { /* quota or private mode — history is best-effort */ }
  listeners.forEach((fn) => fn());
}

/** Record a command. Consecutive duplicates on the same host are collapsed. */
export function pushHistory(command: string, host: string) {
  const trimmed = command.trim();
  if (!trimmed) return;
  const head = entries[0];
  if (head && head.command === trimmed && head.host === host) {
    head.at = Date.now();
    persist();
    return;
  }
  entries = [
    { id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`, command: trimmed, host, at: Date.now() },
    ...entries,
  ].slice(0, MAX_ENTRIES);
  persist();
}

export function getHistory(): HistoryEntry[] {
  return entries;
}

export function clearHistory() {
  entries = [];
  persist();
}

export function removeHistory(id: string) {
  entries = entries.filter((e) => e.id !== id);
  persist();
}

export function subscribeHistory(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}
