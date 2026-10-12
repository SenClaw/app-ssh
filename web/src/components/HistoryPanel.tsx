import { useMemo, useState, useSyncExternalStore } from 'react';
import { Button, Empty, Input, Popconfirm, Tooltip } from 'antd';
import { PlayCircleOutlined, DeleteOutlined, SearchOutlined } from '@ant-design/icons';
import { getHistory, subscribeHistory, clearHistory, removeHistory } from '../history';
import { useAppTheme } from '../theme';

interface Props {
  canSend: boolean;
  onPaste: (script: string) => void;
  onRun: (script: string) => void;
}

/** "3m ago" / "2h ago" / a date once it stops being today's news. */
function ago(at: number): string {
  const secs = Math.max(0, Math.floor((Date.now() - at) / 1000));
  if (secs < 60) return 'just now';
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`;
  return new Date(at).toLocaleDateString();
}

export function HistoryPanel({ canSend, onPaste, onRun }: Props) {
  const { palette } = useAppTheme();
  const entries = useSyncExternalStore(subscribeHistory, getHistory);
  const [query, setQuery] = useState('');

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return entries;
    return entries.filter((e) =>
      e.command.toLowerCase().includes(q) || e.host.toLowerCase().includes(q));
  }, [entries, query]);

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
      <div style={{
        padding: '12px 12px 8px', display: 'flex', alignItems: 'center', gap: 8,
        borderBottom: `1px solid ${palette.border}`,
      }}>
        <Input
          allowClear
          prefix={<SearchOutlined style={{ color: palette.textMuted }} />}
          placeholder="Search history"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          style={{ background: palette.inputBg, borderColor: palette.border, color: palette.text }}
        />
        <Popconfirm title="Clear all history?" onConfirm={clearHistory} okText="Clear"
          okButtonProps={{ danger: true }}>
          <Tooltip title="Clear history">
            <Button type="text" icon={<DeleteOutlined />} style={{ color: palette.textMuted }} />
          </Tooltip>
        </Popconfirm>
      </div>

      <div style={{ flex: 1, overflowY: 'auto', padding: '8px 0' }}>
        {filtered.length === 0 ? (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description={<span style={{ color: palette.textMuted, fontSize: 12 }}>
              {query ? 'No matching commands' : 'Commands you run appear here'}
            </span>}
            style={{ marginTop: 32 }}
          />
        ) : (
          filtered.map((e) => (
            <HistoryRow
              key={e.id}
              command={e.command}
              host={e.host}
              when={ago(e.at)}
              canSend={canSend}
              onPaste={() => onPaste(e.command)}
              onRun={() => onRun(e.command)}
              onRemove={() => removeHistory(e.id)}
            />
          ))
        )}
      </div>
    </div>
  );
}

function HistoryRow({ command, host, when, canSend, onPaste, onRun, onRemove }: {
  command: string; host: string; when: string; canSend: boolean;
  onPaste: () => void; onRun: () => void; onRemove: () => void;
}) {
  const { palette } = useAppTheme();
  const [hover, setHover] = useState(false);

  return (
    <div
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      onClick={() => canSend && onPaste()}
      style={{
        padding: '8px 12px', position: 'relative',
        cursor: canSend ? 'pointer' : 'default',
        background: hover ? palette.elevated : 'transparent',
        opacity: canSend ? 1 : 0.6,
      }}
    >
      <div style={{
        color: palette.text, fontSize: 12, fontFamily: 'Menlo, Monaco, monospace',
        overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', paddingRight: hover ? 56 : 0,
      }}>
        {command}
      </div>
      <div style={{ marginTop: 2, fontSize: 10, color: palette.textMuted }}>
        {host} · {when}
      </div>

      {hover && (
        <div onClick={(e) => e.stopPropagation()}
          style={{
            position: 'absolute', top: 6, right: 8, display: 'flex', gap: 2,
            background: palette.elevated, borderRadius: 6, padding: 2,
          }}>
          <Tooltip title={canSend ? 'Run again' : 'No terminal open'}>
            <Button type="text" size="small" disabled={!canSend} icon={<PlayCircleOutlined />}
              onClick={onRun} style={{ color: canSend ? '#22c55e' : palette.textMuted }} />
          </Tooltip>
          <Tooltip title="Remove">
            <Button type="text" size="small" icon={<DeleteOutlined />} onClick={onRemove}
              style={{ color: palette.textMuted }} />
          </Tooltip>
        </div>
      )}
    </div>
  );
}
