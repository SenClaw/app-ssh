import { useMemo, useState } from 'react';
import { Empty, Input } from 'antd';
import { DesktopOutlined, SearchOutlined } from '@ant-design/icons';
import type { Host } from '../types';
import { useAppTheme } from '../theme';

interface Props {
  hosts: Host[];
  onConnect: (host: Host) => void;
}

/** One-click launcher: open a terminal for a saved host without leaving the tab. */
export function QuickConnectPanel({ hosts, onConnect }: Props) {
  const { palette } = useAppTheme();
  const [query, setQuery] = useState('');

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return hosts;
    return hosts.filter((h) =>
      (h.name ?? '').toLowerCase().includes(q) ||
      h.host.toLowerCase().includes(q) ||
      h.user.toLowerCase().includes(q));
  }, [hosts, query]);

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
      <div style={{ padding: '12px 12px 8px', borderBottom: `1px solid ${palette.border}` }}>
        <Input
          allowClear
          prefix={<SearchOutlined style={{ color: palette.textMuted }} />}
          placeholder="Search hosts"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          style={{ background: palette.inputBg, borderColor: palette.border, color: palette.text }}
        />
      </div>

      <div style={{ flex: 1, overflowY: 'auto', padding: '8px 0' }}>
        {filtered.length === 0 ? (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description={<span style={{ color: palette.textMuted, fontSize: 12 }}>
              {query ? 'No matching hosts' : 'No hosts saved yet'}
            </span>}
            style={{ marginTop: 32 }}
          />
        ) : (
          filtered.map((h) => <HostRow key={h.id} host={h} onConnect={() => onConnect(h)} />)
        )}
      </div>
    </div>
  );
}

function HostRow({ host, onConnect }: { host: Host; onConnect: () => void }) {
  const { palette } = useAppTheme();
  const [hover, setHover] = useState(false);

  return (
    <div
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      onClick={onConnect}
      title="Open a terminal"
      style={{
        padding: '10px 12px', cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 10,
        background: hover ? palette.elevated : 'transparent',
      }}
    >
      <div style={{
        background: '#f97316', borderRadius: '50%', width: 26, height: 26, flexShrink: 0,
        display: 'flex', alignItems: 'center', justifyContent: 'center', color: '#fff', fontSize: 12,
      }}>
        <DesktopOutlined />
      </div>
      <div style={{ minWidth: 0 }}>
        <div style={{
          color: palette.text, fontSize: 13, fontWeight: 500,
          overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
        }}>
          {host.name || host.host}
        </div>
        <div style={{ color: palette.textMuted, fontSize: 11 }}>
          {host.user}@{host.host}:{host.port}
        </div>
      </div>
    </div>
  );
}
