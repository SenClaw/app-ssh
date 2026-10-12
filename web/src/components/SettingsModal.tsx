import { useEffect, useState } from 'react';
import { Button, Modal, Form, Select, Space, Tabs, message, Spin, Typography, Radio } from 'antd';
import { useAppTheme } from '../theme';

const { Text } = Typography;

/// Commands the bundled SSH skills actually run — keep in sync with
/// apps/ssh-manager/skills/*/SKILL.md (ssh-reporting's system report:
/// disk/mem/cpu/network) plus the identity probes agents habitually issue.
const SKILL_COMMANDS = [
  'df', 'free', 'uptime', 'top', 'ip', 'ping',
  'uname', 'ps', 'ss', 'whoami', 'hostname',
];

/// Safe read-only diagnostics for one-click allowing. First-token matching
/// means anything that can mutate state (rm, dd, systemctl, docker, ...) is
/// deliberately NOT in this list — add those explicitly if you accept the risk.
const BASIC_COMMANDS = [
  'ls', 'pwd', 'cat', 'head', 'tail', 'grep', 'find', 'which',
  'echo', 'date', 'id', 'env', 'du', 'stat', 'wc', 'w', 'who',
  'last', 'lsblk', 'lscpu', 'journalctl', 'netstat',
  ...SKILL_COMMANDS,
];

const dedup = (xs: string[]) => Array.from(new Set(xs));

/// Grouped dropdown suggestions for the allowlist picker.
const ALLOW_OPTIONS = [
  {
    label: 'From SSH skills',
    options: SKILL_COMMANDS.map((c) => ({ value: c, label: c })),
  },
  {
    label: 'Basic (read-only)',
    options: BASIC_COMMANDS.filter((c) => !SKILL_COMMANDS.includes(c))
      .sort()
      .map((c) => ({ value: c, label: c })),
  },
];

interface Settings {
  theme: string;
  log_retention_seconds: number;
  ssh_command_policy: 'off' | 'allowlist' | 'denylist' | string;
  ssh_allowed_commands: string[];
  ssh_denied_commands: string[];
}

interface Props {
  open: boolean;
  onClose: () => void;
}

const RETENTION_OPTIONS = [
  { value: 0, label: 'Never (manual only)' },
  { value: 3600, label: '1 hour' },
  { value: 6 * 3600, label: '6 hours' },
  { value: 24 * 3600, label: '24 hours' },
  { value: 7 * 24 * 3600, label: '7 days' },
  { value: 30 * 24 * 3600, label: '30 days' },
];

const THEMES = [
  { value: 'dark', label: 'Dark (default)' },
  { value: 'midnight', label: 'Midnight blue' },
  { value: 'slate', label: 'Slate gray' },
];

export const SettingsModal: React.FC<Props> = ({ open, onClose }) => {
  const { palette } = useAppTheme();
  const [loading, setLoading] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);

  useEffect(() => {
    if (!open) return;
    setLoading(true);
    fetch('./api/settings')
      .then(r => r.json())
      .then((s: Settings) => setSettings(s))
      .catch(() => message.error('Failed to load settings'))
      .finally(() => setLoading(false));
  }, [open]);

  const persist = async (next: Settings) => {
    setSettings(next);
    try {
      await fetch('./api/settings', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(next),
      });
      localStorage.setItem('ssh-theme', next.theme);
    } catch {
      message.error('Failed to save settings');
    }
  };

  const body = !settings ? (
    <Spin />
  ) : (
    <Tabs
      defaultActiveKey="general"
      items={[
        {
          key: 'general',
          label: 'General',
          children: (
            <Form layout="vertical">
              <Form.Item label="Theme">
                <Select
                  value={settings.theme}
                  onChange={(v) => persist({ ...settings, theme: v })}
                  options={THEMES}
                  style={{ width: '100%' }}
                />
                <Text type="secondary">Theme presets affect the sidebar/header accent colors.</Text>
              </Form.Item>
              <Form.Item label="Auto-clear logs">
                <Select
                  value={settings.log_retention_seconds}
                  onChange={(v) => persist({ ...settings, log_retention_seconds: v })}
                  options={RETENTION_OPTIONS}
                  style={{ width: '100%' }}
                />
                <Text type="secondary">
                  Server sweeps every 30s and drops entries older than this. 0 = keep until manual clear.
                </Text>
              </Form.Item>
            </Form>
          ),
        },
        {
          key: 'mcp',
          label: 'MCP Access',
          children: (
            <div>
              <div style={{ color: palette.text, fontWeight: 500, marginBottom: 4 }}>
                ssh_execute_command — per-command policy
              </div>
              <Text type="secondary" style={{ display: 'block', marginBottom: 12 }}>
                Limit which shell commands the AI agent can run via SSH.
                Matching is on the first token (e.g. <code>rm</code> matches <code>rm -rf /tmp</code>).
                Per-tool MCP permissions are managed by SenClaw itself.
              </Text>

              <Form layout="vertical">
                <Form.Item label="Policy">
                  <Radio.Group
                    value={settings.ssh_command_policy}
                    onChange={(e) => persist({ ...settings, ssh_command_policy: e.target.value })}
                  >
                    <Radio.Button value="off">Off (allow all)</Radio.Button>
                    <Radio.Button value="allowlist">Allowlist only</Radio.Button>
                    <Radio.Button value="denylist">Denylist</Radio.Button>
                  </Radio.Group>
                </Form.Item>

                {settings.ssh_command_policy === 'allowlist' && (
                  <Form.Item
                    label="Allowed commands"
                    extra="Only these commands can run. Pick from the suggestions or type any other command and press Enter to add it."
                  >
                    <Select
                      mode="tags"
                      value={settings.ssh_allowed_commands}
                      onChange={(v) => persist({ ...settings, ssh_allowed_commands: dedup(v) })}
                      tokenSeparators={[',', ' ']}
                      options={ALLOW_OPTIONS}
                      placeholder="ls, pwd, df, free, uname, whoami, ..."
                      style={{ width: '100%' }}
                    />
                    <Space style={{ marginTop: 8 }} wrap>
                      <Button
                        size="small"
                        onClick={() =>
                          persist({
                            ...settings,
                            ssh_allowed_commands: dedup([
                              ...settings.ssh_allowed_commands,
                              ...SKILL_COMMANDS,
                            ]),
                          })
                        }
                      >
                        + SSH-skill commands
                      </Button>
                      <Button
                        size="small"
                        onClick={() =>
                          persist({
                            ...settings,
                            ssh_allowed_commands: dedup([
                              ...settings.ssh_allowed_commands,
                              ...BASIC_COMMANDS,
                            ]),
                          })
                        }
                      >
                        + All basic commands
                      </Button>
                      <Button
                        size="small"
                        danger
                        disabled={settings.ssh_allowed_commands.length === 0}
                        onClick={() =>
                          persist({ ...settings, ssh_allowed_commands: [] })
                        }
                      >
                        Clear
                      </Button>
                    </Space>
                  </Form.Item>
                )}

                {settings.ssh_command_policy === 'denylist' && (
                  <Form.Item
                    label="Denied commands"
                    extra="These commands are blocked; everything else is allowed."
                  >
                    <Select
                      mode="tags"
                      value={settings.ssh_denied_commands}
                      onChange={(v) => persist({ ...settings, ssh_denied_commands: v })}
                      tokenSeparators={[',', ' ']}
                      placeholder="rm, dd, mkfs, shutdown, reboot, ..."
                      style={{ width: '100%' }}
                    />
                  </Form.Item>
                )}
              </Form>
            </div>
          ),
        },
      ]}
    />
  );

  return (
    <Modal
      title="Settings"
      open={open}
      onCancel={onClose}
      footer={null}
      width={680}
      destroyOnClose
    >
      {loading ? <Spin /> : body}
    </Modal>
  );
};
