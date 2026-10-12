import React, { useEffect } from 'react';
import { Form, Input, InputNumber, Button, Space, Typography, Divider } from 'antd';
import { CloseOutlined } from '@ant-design/icons';
import type { Host, KeychainItem } from '../types';
import { Select } from 'antd';
import { useAppTheme } from '../theme';

const { Title, Text } = Typography;

interface HostDetailsProps {
  host: Host | null;
  onSave: (host: Host) => void;
  onDelete: (id: string) => void;
  onConnect: (host: Host) => void;
  onClose?: () => void;
}

export const HostDetails: React.FC<HostDetailsProps> = ({ host, onSave, onDelete, onConnect, onClose }) => {
  const { palette } = useAppTheme();
  const [form] = Form.useForm();
  const [keychainItems, setKeychainItems] = React.useState<KeychainItem[]>([]);

  useEffect(() => {
    fetch('./api/keychain')
      .then(res => res.json())
      .then(data => setKeychainItems(data))
      .catch(err => console.error('Failed to fetch keychain', err));
  }, []);
  
  useEffect(() => {
    if (host) {
      form.setFieldsValue(host);
    } else {
      form.resetFields();
      form.setFieldsValue({ port: 22 });
    }
  }, [host, form]);

  const handleSubmit = (values: any) => {
    onSave({
      ...values,
      id: host?.id || '',
      tags: [],
    });
  };

  return (
    <div style={{ padding: '24px', height: '100%', backgroundColor: palette.containerBg, color: palette.text }}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 16 }}>
        <Title level={4} style={{ color: palette.text, margin: 0 }}>
          {host ? 'Edit Host' : 'New Host'}
        </Title>
        {onClose && (
          <Button
            type="text"
            icon={<CloseOutlined />}
            onClick={onClose}
            aria-label="Close"
            style={{ color: palette.textMuted }}
          />
        )}
      </div>
      
      <Form
        form={form}
        layout="vertical"
        onFinish={handleSubmit}
        initialValues={{ port: 22 }}
      >
        <Form.Item
          name="name"
          label={<span style={{ color: palette.textMuted }}>Alias</span>}
          rules={[{ required: true, message: 'Please enter a name' }]}
        >
          <Input placeholder="e.g. Production Server" style={{ backgroundColor: palette.inputBg, color: palette.text, borderColor: palette.border }} />
        </Form.Item>
        
        <Space style={{ display: 'flex', marginBottom: 8 }} align="baseline">
          <Form.Item
            name="host"
            label={<span style={{ color: palette.textMuted }}>Hostname or IP</span>}
            rules={[{ required: true, message: 'Please enter hostname' }]}
            style={{ flex: 1 }}
          >
            <Input placeholder="192.168.1.1" style={{ backgroundColor: palette.inputBg, color: palette.text, borderColor: palette.border, minWidth: 200 }} />
          </Form.Item>
          
          <Form.Item
            name="port"
            label={<span style={{ color: palette.textMuted }}>Port</span>}
            rules={[{ required: true, message: 'Port is required' }]}
          >
            <InputNumber min={1} max={65535} style={{ backgroundColor: palette.inputBg, color: palette.text, borderColor: palette.border, width: 80 }} />
          </Form.Item>
        </Space>
        
        <Divider style={{ borderColor: palette.border, margin: '12px 0' }} />
        <Text style={{ color: palette.textMuted, display: 'block', marginBottom: 16 }}>Credentials</Text>
        
        <Form.Item
          name="user"
          label={<span style={{ color: palette.textMuted }}>Username</span>}
          rules={[{ required: true, message: 'Please enter username' }]}
        >
          <Input placeholder="root" style={{ backgroundColor: palette.inputBg, color: palette.text, borderColor: palette.border }} />
        </Form.Item>
        
        <Form.Item
          name="password"
          label={<span style={{ color: palette.textMuted }}>Password</span>}
        >
          <Input.Password placeholder="Password (if not using Keychain)" style={{ backgroundColor: palette.inputBg, color: palette.text, borderColor: palette.border }} />
        </Form.Item>
        
        <Form.Item
          name="keychain_id"
          label={<span style={{ color: palette.textMuted }}>Use Keychain Credential</span>}
        >
          <Select 
            placeholder="Select a credential (optional)" 
            allowClear
            dropdownStyle={{ backgroundColor: palette.containerBg, color: palette.text }}
            style={{ width: '100%' }}
          >
            {keychainItems.map(item => (
              <Select.Option key={item.id} value={item.id}>
                {item.name} ({item.item_type})
              </Select.Option>
            ))}
          </Select>
        </Form.Item>

        <Form.Item style={{ marginTop: 32 }}>
          <Button type="primary" htmlType="submit" block style={{ backgroundColor: '#3b82f6' }}>
            Save Host
          </Button>
        </Form.Item>
        
        {host && (
          <Space direction="vertical" style={{ width: '100%' }}>
            <Button 
              type="primary" 
              block 
              onClick={() => onConnect(host)}
              style={{ backgroundColor: '#10b981', borderColor: '#10b981' }}
            >
              Connect
            </Button>
            <Button 
              danger 
              block 
              type="text" 
              onClick={() => onDelete(host.id)}
            >
              Delete Host
            </Button>
          </Space>
        )}
      </Form>
    </div>
  );
};
