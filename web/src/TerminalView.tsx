import { useEffect, useRef } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { AttachAddon } from '@xterm/addon-attach';
import '@xterm/xterm/css/xterm.css';
import type { Host } from './types';
import { useAppTheme } from './theme';
import { registerTerminal } from './terminal-bus';
import { pushHistory } from './history';

interface TerminalViewProps {
  host: Host;
  isActive: boolean;
  /** Tab this terminal belongs to — the address the right sidebar sends to. */
  tabId: string;
}

export function TerminalView({ host, isActive, tabId }: TerminalViewProps) {
  const { palette, appearance } = useAppTheme();
  // Typography is read once per connection and then patched live, so changing
  // the font never tears down the SSH session.
  const appearanceRef = useRef(appearance);
  appearanceRef.current = appearance;
  const terminalRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);

  const fitAddonRef = useRef<FitAddon | null>(null);

  useEffect(() => {
    if (!terminalRef.current) return;

    const term = new Terminal({
      theme: {
        background: palette.terminalBg,
        foreground: '#e2e8f0',
        cursor: '#38bdf8',
        selectionBackground: 'rgba(56, 189, 248, 0.3)',
        black: '#0f172a',
        red: '#ef4444',
        green: '#22c55e',
        yellow: '#eab308',
        blue: '#3b82f6',
        magenta: '#d946ef',
        cyan: '#06b6d4',
        white: '#f8fafc',
      },
      fontFamily: appearanceRef.current.fontFamily,
      fontSize: appearanceRef.current.fontSize,
      lineHeight: 1.5,
      cursorBlink: true,
      allowTransparency: true,
    });
    
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    fitAddonRef.current = fitAddon;
    
    term.open(terminalRef.current);

    // Fit before the socket opens: the PTY is created at whatever size the URL
    // carries, and a shell that starts at the wrong width wraps its own prompt
    // in the middle of the line.
    try { fitAddon.fit(); } catch(e) {}

    // Again on the next tick, for a container that had not laid out yet.
    setTimeout(() => {
      try { fitAddon.fit(); } catch(e) {}
    }, 10);
    
    termRef.current = term;

    term.writeln(`\x1b[36mConnecting to ${host.user}@${host.host}...\x1b[0m`);

    // We assume the app is running in a browser with a standard origin.
    // If the API proxy strips this, we can use relative paths or determine the correct base.
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    
    // Construct the websocket URL based on the current window location to ensure it works through the space proxy
    // In senclaw spaces, requests go to `/api/space/apps/ssh-manager/proxy/`
    // We just replace `http` with `ws` and append `api/ws/terminal/:id`
    
    // The current path could be `/api/space/apps/ssh-manager/proxy/`
    // We need to resolve `api/ws/terminal/:id` relative to this.
    // fetch('./api/...') resolves correctly. For WS, we need an absolute URL.
    const baseUrl = new URL('./api', window.location.href);
    baseUrl.protocol = protocol;
    
    const wsUrl = `${baseUrl.href}/ws/terminal/${host.id}?cols=${term.cols}&rows=${term.rows}`;
    const socket = new WebSocket(wsUrl);
    // Shell output arrives as raw bytes so a UTF-8 character split across two
    // SSH packets still decodes; xterm needs the frames as ArrayBuffers.
    socket.binaryType = 'arraybuffer';

    /** Tell the remote PTY its new size. Binary frame = control channel;
     *  keystrokes travel as text, so the two are never confused. */
    const sendSize = (cols: number, rows: number) => {
      if (socket.readyState !== WebSocket.OPEN) return;
      socket.send(new TextEncoder().encode(JSON.stringify({ type: 'resize', cols, rows })));
    };

    const resizeSub = term.onResize(({ cols, rows }) => sendSize(cols, rows));

    // Snippets are injected through `term.input` rather than the raw socket, so
    // they travel the same path as a keystroke: AttachAddon forwards them and
    // the line buffer below sees them, which is what puts them in history.
    let unregister = () => {};

    socket.onopen = () => {
      term.writeln('\x1b[32mConnected.\x1b[0m');
      const attachAddon = new AttachAddon(socket);
      term.loadAddon(attachAddon);
      // A fit that landed while the socket was still connecting never reached
      // the server; the PTY only knows what the URL told it.
      sendSize(term.cols, term.rows);
      unregister = registerTerminal(tabId, (data) => term.input(data));
    };

    socket.onerror = () => {
      term.writeln('\r\n\x1b[31mWebSocket Connection Error\x1b[0m');
    };

    socket.onclose = (e) => {
      term.writeln(`\r\n\x1b[33mConnection closed (code: ${e.code})\x1b[0m`);
    };

    // Watch the container, not the window: this pane also changes size when a
    // tab bar appears or the desktop shell relays out, and every one of those
    // has to reach the remote PTY or the shell wraps at the old width.
    const observer = new ResizeObserver(() => {
      const el = terminalRef.current;
      // A hidden tab measures 0 — fitting against that would send a 1x1 PTY.
      if (!el || !el.clientWidth || !el.clientHeight) return;
      try { fitAddon.fit(); } catch(e) {}
    });
    observer.observe(terminalRef.current);

    // Reconstruct submitted command lines from keystrokes. The backend never
    // sees interactive input (it is piped straight through the websocket), so
    // this is the only place history can be observed.
    const hostLabel = `${host.user}@${host.host}`;
    let lineBuffer = '';
    const dataSub = term.onData((chunk) => {
      for (const ch of chunk) {
        if (ch === '\r' || ch === '\n') {
          pushHistory(lineBuffer, hostLabel);
          lineBuffer = '';
        } else if (ch === '\x7f' || ch === '\b') {
          lineBuffer = lineBuffer.slice(0, -1);
        } else if (ch === '\x03' || ch === '\x15') {
          // Ctrl-C / Ctrl-U discard the line before the shell ever runs it.
          lineBuffer = '';
        } else if (ch >= ' ') {
          lineBuffer += ch;
        }
      }
    });

    const mcpLogHandler = (e: any) => {
      if (e.detail.host_id === host.id) {
        term.writeln(`\r\n\x1b[33m[AI Agent executed: ${e.detail.command}]\x1b[0m\r\n`);
        const output = e.detail.output || '';
        output.split('\n').forEach((line: string) => {
          term.writeln(line.replace(/\r/g, ''));
        });
        term.writeln(`\r\n`);
      }
    };
    window.addEventListener('mcp-log', mcpLogHandler);

    return () => {
      unregister();
      observer.disconnect();
      window.removeEventListener('mcp-log', mcpLogHandler);
      dataSub.dispose();
      resizeSub.dispose();
      socket.close();
      term.dispose();
    };
  }, [host, tabId]);

  // Live typography updates: mutate the running terminal and refit, so the row
  // and column count the remote PTY was told about stays truthful.
  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    term.options.fontSize = appearance.fontSize;
    term.options.fontFamily = appearance.fontFamily;
    try { fitAddonRef.current?.fit(); } catch (e) { /* hidden tab measures 0 */ }
  }, [appearance.fontSize, appearance.fontFamily]);

  useEffect(() => {
    if (isActive && fitAddonRef.current) {
      // Delay fit to ensure DOM has updated its display property
      setTimeout(() => {
        try {
          fitAddonRef.current?.fit();
        } catch (e) {}
      }, 50);
    }
  }, [isActive]);

  return (
    <div style={{ 
      width: '100%', 
      height: '100%', 
      display: 'flex',
      flex: 1,
      overflow: 'hidden',
      backgroundColor: palette.terminalBg,
    }}>
      <div ref={terminalRef} style={{ width: '100%', height: '100%', flex: 1, padding: '8px' }} />
    </div>
  );
}
