import { useSyncExternalStore } from 'react';
import { Tooltip } from 'antd';
import {
  RocketOutlined, CodeOutlined, ClockCircleOutlined, BgColorsOutlined,
} from '@ant-design/icons';
import type { Host } from '../types';
import { useAppTheme } from '../theme';
import { hasTerminal, sendToTerminal, subscribeTerminals } from '../terminal-bus';
import { SnippetsPanel } from './SnippetsPanel';
import { HistoryPanel } from './HistoryPanel';
import { AppearancePanel } from './AppearancePanel';
import { QuickConnectPanel } from './QuickConnectPanel';

export type SidebarPanel = 'hosts' | 'snippets' | 'history' | 'appearance';

interface Props {
  panel: SidebarPanel;
  onPanelChange: (panel: SidebarPanel) => void;
  hosts: Host[];
  onConnect: (host: Host) => void;
  /** The terminal tab currently in front, or null when it is Vaults/SFTP. */
  activeTerminal: { tabId: string; label: string } | null;
}

const TABS: { key: SidebarPanel; icon: React.ReactNode; title: string }[] = [
  { key: 'hosts', icon: <RocketOutlined />, title: 'Quick connect' },
  { key: 'snippets', icon: <CodeOutlined />, title: 'Snippets' },
  { key: 'history', icon: <ClockCircleOutlined />, title: 'History' },
  { key: 'appearance', icon: <BgColorsOutlined />, title: 'Appearance' },
];

/** Enter is CR at a PTY; a script pasted with LFs would land as literal newlines. */
const toTerminalData = (script: string) =>
  script.replace(/[\r\n]+$/, '').replace(/\r\n|\n/g, '\r');

export function RightSidebar({ panel, onPanelChange, hosts, onConnect, activeTerminal }: Props) {
  const { palette } = useAppTheme();
  const tabId = activeTerminal?.tabId ?? null;

  // A tab can be in front while its socket is still connecting — track the
  // socket, not the tab, so "Run" is only offered when it can actually land.
  const canSend = useSyncExternalStore(subscribeTerminals, () => hasTerminal(tabId));

  const paste = (script: string) => sendToTerminal(tabId, toTerminalData(script));
  const run = (script: string) => sendToTerminal(tabId, `${toTerminalData(script)}\r`);

  return (
    <div style={{
      width: 300, flexShrink: 0, height: '100%',
      borderLeft: `1px solid ${palette.border}`,
      background: palette.containerBg,
      display: 'flex', flexDirection: 'column', overflow: 'hidden',
    }}>
      <div style={{
        display: 'flex', gap: 4, padding: 8,
        borderBottom: `1px solid ${palette.border}`,
      }}>
        {TABS.map((t) => {
          const active = panel === t.key;
          return (
            <Tooltip key={t.key} title={t.title} placement="bottom">
              <button
                onClick={() => onPanelChange(t.key)}
                aria-label={t.title}
                aria-pressed={active}
                style={{
                  flex: 1, height: 32, border: 'none', borderRadius: 8, cursor: 'pointer',
                  background: active ? 'rgba(34, 197, 94, 0.18)' : 'transparent',
                  color: active ? '#22c55e' : palette.textMuted,
                  display: 'flex', alignItems: 'center', justifyContent: 'center',
                  fontSize: 15, transition: 'background 0.15s, color 0.15s',
                }}
              >
                {t.icon}
              </button>
            </Tooltip>
          );
        })}
      </div>

      <div style={{ flex: 1, overflow: 'hidden' }}>
        {panel === 'hosts' && <QuickConnectPanel hosts={hosts} onConnect={onConnect} />}
        {panel === 'snippets' && (
          <SnippetsPanel
            targetLabel={activeTerminal?.label ?? null}
            canSend={canSend}
            onPaste={paste}
            onRun={run}
          />
        )}
        {panel === 'history' && <HistoryPanel canSend={canSend} onPaste={paste} onRun={run} />}
        {panel === 'appearance' && <AppearancePanel />}
      </div>
    </div>
  );
}
