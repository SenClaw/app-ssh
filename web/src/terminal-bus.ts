/**
 * Routes text into a live terminal tab.
 *
 * The right sidebar has to reach a PTY it does not own: `TerminalView` holds
 * the socket, the panels only know a tab id. Each mounted terminal registers
 * its writer here, so the sidebar can send a snippet without either side
 * importing the other.
 */

export type TerminalSender = (data: string) => void;

const senders = new Map<string, TerminalSender>();
const listeners = new Set<() => void>();

const notify = () => listeners.forEach((fn) => fn());

/** Called by a terminal once its socket is open. Returns the unregister fn. */
export function registerTerminal(tabId: string, send: TerminalSender): () => void {
  senders.set(tabId, send);
  notify();
  return () => {
    // A remount may already have replaced us; only clear our own entry.
    if (senders.get(tabId) === send) {
      senders.delete(tabId);
      notify();
    }
  };
}

/** True when `tabId` names a terminal whose socket is currently open. */
export function hasTerminal(tabId: string | null | undefined): boolean {
  return !!tabId && senders.has(tabId);
}

/** Write raw data to a terminal as if the user had typed it. */
export function sendToTerminal(tabId: string | null | undefined, data: string): boolean {
  if (!tabId) return false;
  const send = senders.get(tabId);
  if (!send) return false;
  send(data);
  return true;
}

/** Subscribe to terminals connecting/disconnecting (for enabling send buttons). */
export function subscribeTerminals(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}
