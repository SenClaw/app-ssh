import { useEffect, useMemo, useRef, useState } from 'react';
import { Button, Input, Select, Space, Tag, Tooltip, message } from 'antd';
import { ClearOutlined, ReloadOutlined } from '@ant-design/icons';
import { useAppTheme } from '../theme';

interface LogEntry {
  id: number;
  ts: number;
  level: 'info' | 'warn' | 'error' | string;
  source: 'ui' | 'mcp' | 'ssh' | 'system' | string;
  action: string;
  host: string | null;
  message: string;
  meta?: any;
}

const LEVEL_COLOR: Record<string, string> = {
  info: '#3b82f6',
  warn: '#f59e0b',
  error: '#ef4444',
};

const SOURCE_COLOR: Record<string, string> = {
  ui: '#8b5cf6',
  mcp: '#10b981',
  ssh: '#f97316',
  tunnel: '#0ea5e9',
  rfs: '#14b8a6',
  guard: '#eab308',
  system: '#9ca3af',
};

const fmtTime = (ts: number) => {
  const d = new Date(ts);
  return d.toLocaleTimeString(undefined, { hour12: false }) + '.' + String(d.getMilliseconds()).padStart(3, '0');
};

export const LogsView: React.FC = () => {
  const { palette } = useAppTheme();
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [filterSource, setFilterSource] = useState<string | undefined>();
  const [filterLevel, setFilterLevel] = useState<string | undefined>();
  const [search, setSearch] = useState('');
  const [autoScroll, setAutoScroll] = useState(true);
  const listRef = useRef<HTMLDivElement>(null);

  const fetchInitial = async () => {
    try {
      const res = await fetch('./api/logs?limit=500');
      const data: LogEntry[] = await res.json();
      setEntries(data);
    } catch {
      message.error('Failed to load logs');
    }
  };

  useEffect(() => {
    fetchInitial();
    const es = new EventSource('./api/logs/stream');
    es.addEventListener('log', (e) => {
      try {
        const entry: LogEntry = JSON.parse((e as MessageEvent).data);
        setEntries((prev) => {
          const next = [...prev, entry];
          return next.length > 1000 ? next.slice(next.length - 1000) : next;
        });
      } catch {}
    });
    es.onerror = () => {
      // browser auto-reconnects; nothing to do
    };
    return () => es.close();
  }, []);

  useEffect(() => {
    if (autoScroll && listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight;
    }
  }, [entries, autoScroll]);

  const visible = useMemo(() => {
    const q = search.trim().toLowerCase();
    return entries.filter((e) => {
      if (filterSource && e.source !== filterSource) return false;
      if (filterLevel && e.level !== filterLevel) return false;
      if (q) {
        const hay = `${e.action} ${e.message} ${e.host ?? ''}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  }, [entries, filterSource, filterLevel, search]);

  const handleClear = async () => {
    try {
      await fetch('./api/logs', { method: 'DELETE' });
      setEntries([]);
      message.success('Cleared');
    } catch {
      message.error('Failed to clear');
    }
  };

  return (
    <div style={{ padding: 16, height: '100%', display: 'flex', flexDirection: 'column', color: palette.text }}>
      <Space style={{ marginBottom: 12, flexWrap: 'wrap' }}>
        <Input.Search
          placeholder="Search action / message / host"
          allowClear
          onSearch={setSearch}
          onChange={(e) => !e.target.value && setSearch('')}
          style={{ width: 280 }}
        />
        <Select
          placeholder="Source"
          allowClear
          style={{ width: 130 }}
          onChange={setFilterSource}
          options={[
            { value: 'ui', label: 'UI' },
            { value: 'mcp', label: 'MCP' },
            { value: 'ssh', label: 'SSH' },
            { value: 'tunnel', label: 'Tunnel' },
            { value: 'rfs', label: 'Editor' },
            { value: 'system', label: 'System' },
          ]}
        />
        <Select
          placeholder="Level"
          allowClear
          style={{ width: 110 }}
          onChange={setFilterLevel}
          options={[
            { value: 'info', label: 'Info' },
            { value: 'warn', label: 'Warn' },
            { value: 'error', label: 'Error' },
          ]}
        />
        <Tooltip title={autoScroll ? 'Auto-scroll ON' : 'Auto-scroll OFF'}>
          <Button onClick={() => setAutoScroll((v) => !v)} type={autoScroll ? 'primary' : 'default'}>
            Auto-scroll
          </Button>
        </Tooltip>
        <Button icon={<ReloadOutlined />} onClick={fetchInitial}>Reload</Button>
        <Button danger icon={<ClearOutlined />} onClick={handleClear}>Clear</Button>
        <span style={{ color: palette.textMuted, marginLeft: 8 }}>
          {visible.length} / {entries.length}
        </span>
      </Space>

      <div
        ref={listRef}
        style={{
          flex: 1,
          overflowY: 'auto',
          backgroundColor: palette.consoleBg,
          border: `1px solid ${palette.border}`,
          borderRadius: 6,
          padding: 8,
          fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
          fontSize: 12.5,
          lineHeight: 1.6,
        }}
      >
        {visible.length === 0 ? (
          <div style={{ color: '#6b7280', padding: 12 }}>No log entries.</div>
        ) : (
          visible.map((e) => (
            <div
              key={e.id}
              style={{
                display: 'grid',
                gridTemplateColumns: '120px 70px 60px 1fr',
                gap: 8,
                padding: '3px 4px',
                borderBottom: '1px solid rgba(55,65,81,0.4)',
              }}
            >
              <span style={{ color: '#9ca3af' }}>{fmtTime(e.ts)}</span>
              <Tag color={SOURCE_COLOR[e.source] || '#6b7280'} style={{ margin: 0, textAlign: 'center' }}>
                {e.source}
              </Tag>
              <Tag color={LEVEL_COLOR[e.level] || '#6b7280'} style={{ margin: 0, textAlign: 'center' }}>
                {e.level}
              </Tag>
              <span>
                <span style={{ color: '#93c5fd', marginRight: 6 }}>{e.action}</span>
                {e.host && <span style={{ color: '#fbbf24', marginRight: 6 }}>[{e.host}]</span>}
                <span style={{ color: '#e5e7eb' }}>{e.message}</span>
              </span>
            </div>
          ))
        )}
      </div>
    </div>
  );
};
