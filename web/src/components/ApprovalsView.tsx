import { useCallback, useEffect, useState } from 'react';
import { Button, Collapse, Empty, Input, Space, Tag, Tooltip, Typography, message } from 'antd';
import { CheckOutlined, CloseOutlined, DeleteOutlined, ReloadOutlined, SafetyOutlined } from '@ant-design/icons';
import { useAppTheme } from '../theme';
import type { Approval, GuardCheck, GuardDecision, LearnedCommand } from '../types';

const { Text } = Typography;

const SOURCE_LABEL: Record<string, string> = {
  policy: 'Policy',
  learned: 'Learned',
  rules: 'Rules',
  ai: 'AI review',
  system_one: 'System One',
};

const SOURCE_COLOR: Record<string, string> = {
  policy: 'magenta',
  learned: 'green',
  rules: 'orange',
  ai: 'blue',
  system_one: 'purple',
};

const OUTCOME_COLOR: Record<string, string> = {
  pass: '#22c55e',
  raise: '#ef4444',
  block: '#ef4444',
  note: '#9ca3af',
};

const STATUS_COLOR: Record<string, string> = {
  pending: 'gold',
  running: 'processing',
  done: 'success',
  failed: 'error',
  rejected: 'default',
  expired: 'default',
};

const ago = (ts: number) => {
  const s = Math.max(0, Math.round((Date.now() - ts) / 1000));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return new Date(ts).toLocaleString();
};

const post = async (url: string, body?: unknown) => {
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body ?? {}),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
  return data;
};

/** The reasons behind a decision, one line per reviewer. */
export const Checks: React.FC<{ checks: GuardCheck[] }> = ({ checks }) => {
  const { palette } = useAppTheme();
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
      {checks.filter((c) => c.detail).map((c, i) => (
        <div key={i} style={{ display: 'flex', gap: 8, alignItems: 'baseline' }}>
          <Tag color={SOURCE_COLOR[c.source]} style={{ flexShrink: 0, marginInlineEnd: 0 }}>
            {SOURCE_LABEL[c.source] ?? c.source}
          </Tag>
          <span style={{ color: OUTCOME_COLOR[c.outcome] ?? palette.textMuted, fontSize: 12, flexShrink: 0 }}>
            {c.outcome}
          </span>
          <span style={{ color: palette.text, fontSize: 13 }}>{c.detail}</span>
        </div>
      ))}
    </div>
  );
};

const Command: React.FC<{ text: string }> = ({ text }) => {
  const { palette } = useAppTheme();
  return (
    <pre style={{
      margin: 0, padding: '8px 10px', borderRadius: 6, background: palette.consoleBg,
      color: palette.consoleText, fontSize: 13, whiteSpace: 'pre-wrap', wordBreak: 'break-all',
    }}>{text}</pre>
  );
};

export const ApprovalsView: React.FC = () => {
  const { palette } = useAppTheme();
  const [approvals, setApprovals] = useState<Approval[]>([]);
  const [learned, setLearned] = useState<LearnedCommand[]>([]);
  const [notes, setNotes] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [probe, setProbe] = useState('');
  const [probeResult, setProbeResult] = useState<GuardDecision | null>(null);
  const [probing, setProbing] = useState(false);

  const load = useCallback(
    () =>
      Promise.all([
        fetch('./api/guard/approvals').then((r) => r.json()),
        fetch('./api/guard/learned').then((r) => r.json()),
      ])
        .then(([a, l]: [Approval[], LearnedCommand[]]) => {
          setApprovals(a);
          setLearned(l);
        })
        .catch(() => message.error('Failed to load approvals')),
    [],
  );

  useEffect(() => {
    void load();
    // App.tsx re-broadcasts the server's guard_approval events.
    const onEvent = (e: Event) => {
      const a = (e as CustomEvent).detail?.approval as Approval | undefined;
      if (!a) return;
      setApprovals((prev) => [a, ...prev.filter((x) => x.id !== a.id)].sort((x, y) => y.requested_at - x.requested_at));
      if (a.remembered) fetch('./api/guard/learned').then((r) => r.json()).then(setLearned).catch(() => {});
    };
    window.addEventListener('guard-approval', onEvent);
    return () => window.removeEventListener('guard-approval', onEvent);
  }, [load]);

  const decide = async (a: Approval, action: 'approve' | 'remember' | 'reject') => {
    setBusy(a.id);
    try {
      if (action === 'reject') {
        await post(`./api/guard/approvals/${a.id}/reject`, { note: notes[a.id] || null });
      } else {
        await post(`./api/guard/approvals/${a.id}/approve`, { remember: action === 'remember' });
      }
    } catch (e) {
      message.error(String((e as Error).message));
    } finally {
      setBusy(null);
      load();
    }
  };

  const forget = async (command: string) => {
    try {
      const res = await fetch('./api/guard/learned', {
        method: 'DELETE',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ command }),
      });
      if (!res.ok) throw new Error();
      setLearned((prev) => prev.filter((l) => l.command !== command));
    } catch {
      message.error('Failed to remove');
    }
  };

  const runProbe = async () => {
    if (!probe.trim()) return;
    setProbing(true);
    setProbeResult(null);
    try {
      setProbeResult(await post('./api/guard/check', { command: probe }));
    } catch (e) {
      message.error(String((e as Error).message));
    } finally {
      setProbing(false);
    }
  };

  const pending = approvals.filter((a) => a.status === 'pending');
  const history = approvals.filter((a) => a.status !== 'pending');

  const section = (title: string, extra?: React.ReactNode) => (
    <div style={{ display: 'flex', alignItems: 'center', margin: '24px 0 12px', color: palette.textMuted, fontSize: 16 }}>
      <span>{title}</span>
      <span style={{ marginLeft: 'auto' }}>{extra}</span>
    </div>
  );

  const card: React.CSSProperties = {
    border: `1px solid ${palette.border}`, borderRadius: 8, padding: 16, background: palette.containerBg,
    marginBottom: 12, display: 'flex', flexDirection: 'column', gap: 10,
  };

  return (
    <div style={{ padding: 24, overflowY: 'auto', height: '100%', width: '100%' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, color: palette.text, fontSize: 18 }}>
        <SafetyOutlined />
        <span>Command approvals</span>
        <Button type="text" icon={<ReloadOutlined />} onClick={load} style={{ marginLeft: 'auto', color: palette.textMuted }} />
      </div>
      <Text type="secondary">
        Commands the AI agent sent that were not cleared automatically. Approving runs the command on the
        server it was meant for; "Approve &amp; remember" also lets this exact command run without asking next time.
      </Text>

      {section(`Waiting (${pending.length})`)}
      {pending.length === 0 && <Empty description={<span style={{ color: palette.textMuted }}>Nothing is waiting.</span>} />}
      {pending.map((a) => (
        <div key={a.id} style={{ ...card, borderColor: '#f59e0b' }} data-testid="approval-pending">
          <div style={{ display: 'flex', gap: 8, color: palette.textMuted, fontSize: 12 }}>
            <span>{a.target}</span>
            <span>·</span>
            <span>{a.origin === 'mcp' ? 'AI agent' : 'REST API'}</span>
            <span style={{ marginLeft: 'auto' }}>{ago(a.requested_at)}</span>
          </div>
          <Command text={a.command} />
          <Checks checks={a.checks} />
          <Space wrap>
            <Button type="primary" icon={<CheckOutlined />} loading={busy === a.id} onClick={() => decide(a, 'approve')}>
              Approve
            </Button>
            <Tooltip title="Run it, and let this exact command run without asking next time">
              <Button loading={busy === a.id} onClick={() => decide(a, 'remember')}>Approve &amp; remember</Button>
            </Tooltip>
            <Input
              placeholder="Reason (optional)"
              value={notes[a.id] ?? ''}
              onChange={(e) => setNotes((n) => ({ ...n, [a.id]: e.target.value }))}
              style={{ width: 220 }}
            />
            <Button danger icon={<CloseOutlined />} loading={busy === a.id} onClick={() => decide(a, 'reject')}>
              Reject
            </Button>
          </Space>
        </div>
      ))}

      {section('Check a command', <Text type="secondary" style={{ fontSize: 12 }}>Nothing runs and nothing is learned</Text>)}
      <Space.Compact style={{ width: '100%' }}>
        <Input
          placeholder="e.g. docker restart web"
          value={probe}
          onChange={(e) => setProbe(e.target.value)}
          onPressEnter={runProbe}
          style={{ fontFamily: 'monospace' }}
        />
        <Button onClick={runProbe} loading={probing}>Check</Button>
      </Space.Compact>
      {probeResult && (
        <div style={{ ...card, marginTop: 12 }}>
          <div>
            <Tag color={probeResult.action === 'run' ? 'success' : probeResult.action === 'ask' ? 'gold' : 'error'}>
              {probeResult.action === 'run' ? 'Runs at once' : probeResult.action === 'ask' ? 'Waits for the admin' : 'Refused'}
            </Tag>
          </div>
          <Checks checks={probeResult.checks} />
        </div>
      )}

      {section(`Learned commands (${learned.length})`)}
      <Text type="secondary" style={{ display: 'block', marginBottom: 8 }}>
        Exact commands that run without asking: cleared by both the AI review and System One, or approved with "remember".
      </Text>
      {learned.length === 0 && <Empty description={<span style={{ color: palette.textMuted }}>None yet.</span>} />}
      {learned.map((l) => (
        <div key={l.command} style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '6px 0', borderBottom: `1px solid ${palette.border}` }}>
          <Tag color={l.source === 'admin' ? 'magenta' : 'blue'}>{l.source === 'admin' ? 'admin' : 'AI'}</Tag>
          <code style={{ color: palette.text, wordBreak: 'break-all' }}>{l.command}</code>
          <Text type="secondary" style={{ fontSize: 12 }} ellipsis={{ tooltip: l.reason }}>{l.reason}</Text>
          <span style={{ marginLeft: 'auto', color: palette.textMuted, fontSize: 12, flexShrink: 0 }}>{ago(l.added_at)}</span>
          <Tooltip title="Forget: ask again next time">
            <Button type="text" size="small" icon={<DeleteOutlined />} onClick={() => forget(l.command)} style={{ color: palette.textMuted }} />
          </Tooltip>
        </div>
      ))}

      {section(`History (${history.length})`)}
      {history.length === 0 && <Empty description={<span style={{ color: palette.textMuted }}>No decisions yet.</span>} />}
      {history.length > 0 && (
        <Collapse
          items={history.map((a) => ({
            key: a.id,
            label: (
              <span style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                <Tag color={STATUS_COLOR[a.status]}>{a.status}</Tag>
                <code style={{ color: palette.text, wordBreak: 'break-all' }}>{a.command}</code>
                <span style={{ marginLeft: 'auto', color: palette.textMuted, fontSize: 12, flexShrink: 0 }}>
                  {a.target} · {ago(a.finished_at ?? a.decided_at ?? a.requested_at)}
                </span>
              </span>
            ),
            children: (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
                <Checks checks={a.checks} />
                {a.remembered && <Text type="secondary">Approved and remembered.</Text>}
                {a.note && <Text type="secondary">Reason: {a.note}</Text>}
                {a.error && <Text type="danger">{a.error}</Text>}
                {a.output !== null && <Command text={a.output || '(no output)'} />}
              </div>
            ),
          }))}
        />
      )}
    </div>
  );
};
