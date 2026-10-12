import { useEffect, useMemo, useState } from 'react';
import { Button, Input, Empty, Form, Select, Tooltip, Popconfirm, message, Spin } from 'antd';
import {
  CodeOutlined, PlusOutlined, SearchOutlined, ArrowLeftOutlined,
  PlayCircleOutlined, EditOutlined, DeleteOutlined, CloseOutlined,
} from '@ant-design/icons';
import type { Snippet } from '../types';
import { useAppTheme } from '../theme';

interface Props {
  /** Terminal to send into; null when the active tab is not a terminal. */
  targetLabel: string | null;
  canSend: boolean;
  /** Paste the script at the prompt without executing it. */
  onPaste: (script: string) => void;
  /** Paste and press Enter. */
  onRun: (script: string) => void;
}

type Draft = { id?: string; name: string; script: string; description: string; tags: string[] };

const EMPTY_DRAFT: Draft = { name: '', script: '', description: '', tags: [] };

export function SnippetsPanel({ targetLabel, canSend, onPaste, onRun }: Props) {
  const { palette } = useAppTheme();
  const [snippets, setSnippets] = useState<Snippet[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState('');
  const [searchOpen, setSearchOpen] = useState(false);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => { void load(); }, []);

  const load = async () => {
    setLoading(true);
    try {
      const res = await fetch('./api/snippets');
      setSnippets(await res.json());
    } catch {
      message.error('Failed to load snippets');
    } finally {
      setLoading(false);
    }
  };

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return snippets;
    return snippets.filter((s) =>
      s.name.toLowerCase().includes(q) ||
      s.script.toLowerCase().includes(q) ||
      (s.description ?? '').toLowerCase().includes(q) ||
      s.tags.some((t) => t.toLowerCase().includes(q)));
  }, [snippets, query]);

  const save = async () => {
    if (!draft) return;
    const name = draft.name.trim() || draft.script.trim().split('\n')[0];
    if (!draft.script.trim()) {
      message.warning('Script is required');
      return;
    }
    setSaving(true);
    const body = {
      id: draft.id ?? '',
      name,
      script: draft.script,
      description: draft.description.trim() || null,
      tags: draft.tags,
      created_at: 0,
    };
    try {
      const res = await fetch(draft.id ? `./api/snippets/${draft.id}` : './api/snippets', {
        method: draft.id ? 'PUT' : 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      if (!res.ok) throw new Error(String(res.status));
      message.success(draft.id ? 'Snippet updated' : 'Snippet saved');
      setDraft(null);
      await load();
    } catch {
      message.error('Failed to save snippet');
    } finally {
      setSaving(false);
    }
  };

  const remove = async (id: string) => {
    try {
      await fetch(`./api/snippets/${id}`, { method: 'DELETE' });
      setSnippets((prev) => prev.filter((s) => s.id !== id));
    } catch {
      message.error('Failed to delete snippet');
    }
  };

  if (draft) {
    return (
      <SnippetForm
        draft={draft}
        saving={saving}
        onChange={setDraft}
        onCancel={() => setDraft(null)}
        onSave={save}
      />
    );
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
      <div style={{ padding: '12px 12px 8px', display: 'flex', alignItems: 'center', gap: 8 }}>
        <Button
          icon={<PlusOutlined />}
          onClick={() => setDraft({ ...EMPTY_DRAFT })}
          style={{
            flex: 1, textAlign: 'left', background: palette.elevated,
            borderColor: 'transparent', color: palette.text,
          }}
        >
          New Snippet
        </Button>
        <Tooltip title="Search snippets">
          <Button
            type="text"
            icon={<SearchOutlined />}
            onClick={() => { setSearchOpen((v) => !v); if (searchOpen) setQuery(''); }}
            style={{ color: searchOpen ? palette.text : palette.textMuted }}
          />
        </Tooltip>
      </div>

      {searchOpen && (
        <div style={{ padding: '0 12px 8px' }}>
          <Input
            autoFocus
            allowClear
            placeholder="Filter by name, script or tag"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            style={{ background: palette.inputBg, borderColor: palette.border, color: palette.text }}
          />
        </div>
      )}

      <div style={{
        padding: '0 12px 8px', fontSize: 11, color: palette.textMuted,
        borderBottom: `1px solid ${palette.border}`,
      }}>
        {canSend
          ? <>Click to paste into <b style={{ color: palette.text }}>{targetLabel}</b> · ▶ to run</>
          : 'Open a terminal tab to send commands'}
      </div>

      <div style={{ flex: 1, overflowY: 'auto', padding: '8px 0' }}>
        {loading ? (
          <div style={{ textAlign: 'center', padding: 24 }}><Spin /></div>
        ) : filtered.length === 0 ? (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description={
              <span style={{ color: palette.textMuted, fontSize: 12 }}>
                {query ? 'No snippets match' : 'No snippets yet'}
              </span>
            }
            style={{ marginTop: 32 }}
          />
        ) : (
          filtered.map((s) => (
            <SnippetRow
              key={s.id}
              snippet={s}
              canSend={canSend}
              onPaste={() => onPaste(s.script)}
              onRun={() => onRun(s.script)}
              onEdit={() => setDraft({
                id: s.id, name: s.name, script: s.script,
                description: s.description ?? '', tags: s.tags ?? [],
              })}
              onDelete={() => remove(s.id)}
            />
          ))
        )}
      </div>
    </div>
  );
}

function SnippetRow({ snippet, canSend, onPaste, onRun, onEdit, onDelete }: {
  snippet: Snippet;
  canSend: boolean;
  onPaste: () => void;
  onRun: () => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const { palette } = useAppTheme();
  const [hover, setHover] = useState(false);
  // Multi-line scripts collapse to their first line in the list, like Termius.
  const preview = snippet.script.split('\n')[0];
  const extraLines = snippet.script.trim().split('\n').length - 1;

  return (
    <div
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      onClick={() => canSend && onPaste()}
      style={{
        padding: '10px 12px',
        cursor: canSend ? 'pointer' : 'default',
        background: hover ? palette.elevated : 'transparent',
        opacity: canSend ? 1 : 0.6,
        position: 'relative',
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 6, minWidth: 0 }}>
        <CodeOutlined style={{ color: palette.textMuted, fontSize: 12, flexShrink: 0 }} />
        <span style={{
          color: palette.text, fontSize: 13, fontWeight: 500,
          overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
        }}>
          {snippet.name}
        </span>
      </div>
      <div style={{
        marginTop: 4, marginLeft: 18, fontFamily: 'Menlo, Monaco, monospace',
        fontSize: 11, color: palette.textMuted,
        overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
      }}>
        {preview}{extraLines > 0 && ` … +${extraLines} line${extraLines > 1 ? 's' : ''}`}
      </div>

      {hover && (
        <div
          onClick={(e) => e.stopPropagation()}
          style={{
            position: 'absolute', top: 6, right: 8, display: 'flex', gap: 2,
            background: palette.elevated, borderRadius: 6, padding: 2,
          }}
        >
          <Tooltip title={canSend ? 'Run in terminal' : 'No terminal open'}>
            <Button
              type="text" size="small" disabled={!canSend}
              icon={<PlayCircleOutlined />}
              onClick={onRun}
              style={{ color: canSend ? '#22c55e' : palette.textMuted }}
            />
          </Tooltip>
          <Tooltip title="Edit">
            <Button type="text" size="small" icon={<EditOutlined />} onClick={onEdit}
              style={{ color: palette.textMuted }} />
          </Tooltip>
          <Popconfirm
            title="Delete snippet?"
            onConfirm={onDelete}
            okText="Delete"
            okButtonProps={{ danger: true }}
          >
            <Button type="text" size="small" icon={<DeleteOutlined />}
              style={{ color: '#ef4444' }} />
          </Popconfirm>
        </div>
      )}
    </div>
  );
}

function SnippetForm({ draft, saving, onChange, onCancel, onSave }: {
  draft: Draft;
  saving: boolean;
  onChange: (d: Draft) => void;
  onCancel: () => void;
  onSave: () => void;
}) {
  const { palette } = useAppTheme();
  const fieldStyle = { background: palette.inputBg, borderColor: palette.border, color: palette.text };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
      <div style={{
        display: 'flex', alignItems: 'center', gap: 8,
        padding: '10px 12px', borderBottom: `1px solid ${palette.border}`,
      }}>
        <Button type="text" size="small" icon={<ArrowLeftOutlined />} onClick={onCancel}
          style={{ color: palette.textMuted }} />
        <span style={{ color: palette.text, fontWeight: 600, flex: 1 }}>
          {draft.id ? 'Edit Snippet' : 'New Snippet'}
        </span>
        <Button type="primary" size="small" loading={saving} onClick={onSave}>Save</Button>
      </div>

      <div style={{ flex: 1, overflowY: 'auto', padding: 12 }}>
        <Form layout="vertical" size="small">
          <Form.Item label={<span style={{ color: palette.textMuted }}>Name</span>}>
            <Input
              placeholder="Defaults to the first line of the script"
              value={draft.name}
              onChange={(e) => onChange({ ...draft, name: e.target.value })}
              style={fieldStyle}
            />
          </Form.Item>

          <Form.Item
            label={<span style={{ color: palette.textMuted }}>Script *</span>}
            help={<span style={{ color: palette.textMuted, fontSize: 11 }}>
              Each line is sent to the shell as typed.
            </span>}
          >
            <Input.TextArea
              autoFocus
              rows={6}
              placeholder="docker compose up -d"
              value={draft.script}
              onChange={(e) => onChange({ ...draft, script: e.target.value })}
              style={{ ...fieldStyle, fontFamily: 'Menlo, Monaco, monospace', fontSize: 12 }}
            />
          </Form.Item>

          <Form.Item label={<span style={{ color: palette.textMuted }}>Description</span>}>
            <Input.TextArea
              rows={2}
              placeholder="What this snippet does"
              value={draft.description}
              onChange={(e) => onChange({ ...draft, description: e.target.value })}
              style={fieldStyle}
            />
          </Form.Item>

          <Form.Item label={<span style={{ color: palette.textMuted }}>Tags</span>}>
            <Select
              mode="tags"
              placeholder="docker, k8s, cleanup"
              value={draft.tags}
              onChange={(tags) => onChange({ ...draft, tags })}
              style={{ width: '100%' }}
              suffixIcon={<CloseOutlined style={{ display: 'none' }} />}
            />
          </Form.Item>
        </Form>
      </div>
    </div>
  );
}
