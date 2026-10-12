import React, { useState, useEffect, useRef } from 'react';
import { Table, Button, Modal, Form, Input, Select, message, Space, Popconfirm, Tag, Tooltip, InputNumber } from 'antd';
import { PlusOutlined, DeleteOutlined, EditOutlined, ApiOutlined, PlayCircleOutlined, StopOutlined, ReloadOutlined, WarningOutlined } from '@ant-design/icons';
import type { PortForwardingRule, TunnelActionResponse, Host } from '../types';
import { useAppTheme } from '../theme';

/** The status column is only worth anything if it keeps up with reality — a
 *  tunnel can lose its SSH session, or be opened by the agent over MCP, with
 *  nobody touching this screen. */
const POLL_MS = 4000;

const fmtUptime = (seconds: number | null): string => {
  if (seconds === null || seconds === undefined) return '';
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  const rem = m % 60;
  if (h < 24) return rem ? `${h}h ${rem}m` : `${h}h`;
  const d = Math.floor(h / 24);
  return `${d}d ${h % 24}h`;
};

const StatusCell: React.FC<{ rule: PortForwardingRule }> = ({ rule }) => {
  if (rule.status === 'active') {
    return (
      <Tooltip
        title={
          <>
            Listening on {rule.listening_on}<br />
            Up {fmtUptime(rule.uptime_seconds)} · {rule.total_connections} connection(s) since start
          </>
        }
      >
        <Space size={6}>
          <Tag color="green" style={{ margin: 0 }}>Active</Tag>
          <span style={{ fontSize: 12, opacity: 0.6 }}>
            {rule.connections > 0 ? `${rule.connections} open · ` : ''}{fmtUptime(rule.uptime_seconds)}
          </span>
        </Space>
      </Tooltip>
    );
  }

  if (rule.status === 'error') {
    // `running` tells apart "port is held but nothing gets through" from
    // "never came up" — different problems, different next step.
    return (
      <Tooltip title={rule.last_error || 'Tunnel is not forwarding'}>
        <Space size={6}>
          <Tag color="red" icon={<WarningOutlined />} style={{ margin: 0 }}>
            {rule.running ? 'Not forwarding' : 'Failed'}
          </Tag>
          <span style={{ fontSize: 12, opacity: 0.6, maxWidth: 220, display: 'inline-block', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', verticalAlign: 'bottom' }}>
            {rule.last_error}
          </span>
        </Space>
      </Tooltip>
    );
  }

  return <Tag style={{ margin: 0 }}>Inactive</Tag>;
};

export const PortForwardingView: React.FC = () => {
  const { palette } = useAppTheme();
  const [rules, setRules] = useState<PortForwardingRule[]>([]);
  const [hosts, setHosts] = useState<Host[]>([]);
  const [isModalVisible, setIsModalVisible] = useState(false);
  const [editingRule, setEditingRule] = useState<PortForwardingRule | null>(null);
  const [busy, setBusy] = useState<Record<string, boolean>>({});
  const [form] = Form.useForm();
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    fetchRules();
    fetchHosts();
    const timer = setInterval(() => fetchRules(), POLL_MS);
    return () => {
      alive.current = false;
      clearInterval(timer);
    };
  }, []);

  const fetchRules = async (notifyOnError = false) => {
    try {
      const response = await fetch('./api/port-forwarding');
      const data = await response.json();
      if (alive.current) setRules(data);
    } catch (err) {
      if (notifyOnError) message.error('Failed to load port forwarding rules');
    }
  };

  const fetchHosts = async () => {
    try {
      const response = await fetch('./api/hosts');
      const data = await response.json();
      if (alive.current) setHosts(data);
    } catch (err) {
      message.error('Failed to load hosts');
    }
  };

  const handleSave = async (values: any) => {
    try {
      const url = editingRule ? `./api/port-forwarding/${editingRule.id}` : './api/port-forwarding';
      const method = editingRule ? 'PUT' : 'POST';

      const payload = {
        id: editingRule?.id || '',
        name: values.name,
        host_id: values.host_id,
        bind_address: values.bind_address || '127.0.0.1',
        local_port: Number(values.local_port),
        destination_address: values.destination_address,
        destination_port: Number(values.destination_port),
        // The stored flag is the *intent*, so an edit keeps the tunnel the user
        // had switched on — the server restarts it on the new ports.
        active: editingRule?.enabled ?? false,
      };

      const response = await fetch(url, {
        method,
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
      });

      if (!response.ok) throw new Error('save failed');
      const saved: PortForwardingRule = await response.json();

      message.success(editingRule ? 'Rule updated' : 'Rule added');
      // An edit restarts a live tunnel; if it did not come back, say why here
      // rather than leaving it to be noticed in the status column.
      if (saved?.status === 'error' && saved.last_error) {
        message.error({ content: saved.last_error, duration: 8 });
      }
      setIsModalVisible(false);
      fetchRules();
    } catch (err) {
      message.error('Failed to save rule');
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await fetch(`./api/port-forwarding/${id}`, { method: 'DELETE' });
      message.success('Rule deleted');
      fetchRules();
    } catch (err) {
      message.error('Failed to delete rule');
    }
  };

  const callAction = async (rule: PortForwardingRule, action: 'start' | 'stop'): Promise<boolean> => {
    setBusy((b) => ({ ...b, [rule.id]: true }));
    try {
      const response = await fetch(`./api/port-forwarding/${rule.id}/${action}`, { method: 'POST' });
      // The body carries the reason even on a non-2xx — that reason is the
      // whole point, so read it before trusting the status code.
      const data: TunnelActionResponse = await response.json().catch(() => ({ ok: response.ok }));

      if (data.ok) {
        message.success(action === 'start' ? `Tunnel "${rule.name}" is forwarding` : `Tunnel "${rule.name}" stopped`);
      } else {
        message.error({ content: data.error || `Failed to ${action} tunnel "${rule.name}"`, duration: 8 });
      }
      return !!data.ok;
    } catch (err) {
      message.error(`Failed to ${action} tunnel "${rule.name}"`);
      return false;
    } finally {
      setBusy((b) => ({ ...b, [rule.id]: false }));
      fetchRules();
    }
  };

  const restartTunnel = async (rule: PortForwardingRule) => {
    await callAction(rule, 'stop');
    await callAction(rule, 'start');
  };

  const columns = [
    {
      title: 'Name',
      dataIndex: 'name',
      key: 'name',
      render: (text: string) => <><ApiOutlined style={{ marginRight: 8, color: '#3b82f6' }} />{text}</>,
    },
    {
      title: 'Host',
      dataIndex: 'host_id',
      key: 'host_id',
      render: (hostId: string, record: PortForwardingRule) =>
        record.host_name || hosts.find(h => h.id === hostId)?.name || 'Unknown',
    },
    {
      title: 'Local',
      key: 'local',
      render: (_: any, record: PortForwardingRule) => `${record.bind_address}:${record.local_port}`,
    },
    {
      title: 'Remote',
      key: 'remote',
      render: (_: any, record: PortForwardingRule) => `${record.destination_address}:${record.destination_port}`,
    },
    {
      title: 'Status',
      key: 'status',
      render: (_: any, record: PortForwardingRule) => <StatusCell rule={record} />,
    },
    {
      title: 'Actions',
      key: 'actions',
      render: (_: any, record: PortForwardingRule) => (
        <Space>
          {record.running ? (
            <>
              {record.status === 'error' && (
                <Button
                  type="text"
                  icon={<ReloadOutlined />}
                  style={{ color: '#f59e0b' }}
                  loading={busy[record.id]}
                  onClick={() => restartTunnel(record)}
                  title="Restart tunnel"
                />
              )}
              <Button
                type="text"
                icon={<StopOutlined />}
                style={{ color: '#ef4444' }}
                loading={busy[record.id]}
                onClick={() => callAction(record, 'stop')}
                title="Stop tunnel"
              />
            </>
          ) : (
            <Button
              type="text"
              icon={<PlayCircleOutlined />}
              style={{ color: '#10b981' }}
              loading={busy[record.id]}
              onClick={() => callAction(record, 'start')}
              title="Start tunnel"
            />
          )}
          <Button
            type="text"
            icon={<EditOutlined />}
            style={{ color: '#3b82f6' }}
            onClick={() => {
              setEditingRule(record);
              form.setFieldsValue(record);
              setIsModalVisible(true);
            }}
          />
          <Popconfirm
            title={record.running ? 'Delete this rule and close its tunnel?' : 'Delete this rule?'}
            onConfirm={() => handleDelete(record.id)}
          >
            <Button type="text" danger icon={<DeleteOutlined />} />
          </Popconfirm>
        </Space>
      ),
    },
  ];

  return (
    <div style={{ padding: 24, height: '100%', overflowY: 'auto', color: palette.text }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 24 }}>
        <div style={{ fontSize: 20, fontWeight: 500 }}>Port Forwarding</div>
        <Space>
          <Button icon={<ReloadOutlined />} onClick={() => fetchRules(true)}>Refresh</Button>
          <Button
            type="primary"
            icon={<PlusOutlined />}
            onClick={() => {
              setEditingRule(null);
              form.resetFields();
              setIsModalVisible(true);
            }}
          >
            Add Rule
          </Button>
        </Space>
      </div>

      <Table
        dataSource={rules}
        columns={columns}
        rowKey="id"
        pagination={false}
        style={{ background: palette.containerBg, borderRadius: 8 }}
      />

      <Modal
        title={editingRule ? "Edit Rule" : "New Rule"}
        open={isModalVisible}
        onCancel={() => setIsModalVisible(false)}
        footer={null}
        styles={{ body: { backgroundColor: palette.containerBg, color: palette.text }, header: { backgroundColor: palette.containerBg } }}
      >
        <Form form={form} layout="vertical" onFinish={handleSave} initialValues={{ bind_address: '127.0.0.1' }}>
          <Form.Item name="name" label={<span style={{ color: palette.text }}>Rule Name</span>} rules={[{ required: true }]}>
            <Input placeholder="e.g. Database Tunnel" style={{ backgroundColor: palette.inputBg, color: palette.text, borderColor: palette.border }} />
          </Form.Item>
          <Form.Item name="host_id" label={<span style={{ color: palette.text }}>SSH Host</span>} rules={[{ required: true }]}>
            <Select
              placeholder="Select an SSH host to tunnel through"
              dropdownStyle={{ backgroundColor: palette.containerBg, color: palette.text }}
              style={{ width: '100%' }}
            >
              {hosts.map(host => (
                <Select.Option key={host.id} value={host.id}>{host.name}</Select.Option>
              ))}
            </Select>
          </Form.Item>
          <div style={{ display: 'flex', gap: '16px' }}>
            <Form.Item name="bind_address" label={<span style={{ color: palette.text }}>Local Bind Address</span>} rules={[{ required: true }]} style={{ flex: 1 }}>
              <Input placeholder="127.0.0.1" style={{ backgroundColor: palette.inputBg, color: palette.text, borderColor: palette.border }} />
            </Form.Item>
            <Form.Item name="local_port" label={<span style={{ color: palette.text }}>Local Port</span>} rules={[{ required: true }]} style={{ width: '120px' }}>
              <InputNumber min={1} max={65535} style={{ width: '100%', backgroundColor: palette.inputBg, color: palette.text, borderColor: palette.border }} />
            </Form.Item>
          </div>
          <div style={{ display: 'flex', gap: '16px' }}>
            <Form.Item name="destination_address" label={<span style={{ color: palette.text }}>Remote Target Address</span>} rules={[{ required: true }]} style={{ flex: 1 }}>
              <Input placeholder="127.0.0.1" style={{ backgroundColor: palette.inputBg, color: palette.text, borderColor: palette.border }} />
            </Form.Item>
            <Form.Item name="destination_port" label={<span style={{ color: palette.text }}>Remote Port</span>} rules={[{ required: true }]} style={{ width: '120px' }}>
              <InputNumber min={1} max={65535} style={{ width: '100%', backgroundColor: palette.inputBg, color: palette.text, borderColor: palette.border }} />
            </Form.Item>
          </div>
          <div style={{ display: 'flex', justifyContent: 'flex-end', marginTop: 24 }}>
            <Button onClick={() => setIsModalVisible(false)} style={{ marginRight: 8, backgroundColor: 'transparent', color: palette.text }}>Cancel</Button>
            <Button type="primary" htmlType="submit">Save</Button>
          </div>
        </Form>
      </Modal>
    </div>
  );
};
