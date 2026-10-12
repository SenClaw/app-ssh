import React, { useState, useEffect } from 'react';
import { Table, Button, Modal, Form, Input, Select, message, Space, Popconfirm } from 'antd';
import { PlusOutlined, DeleteOutlined, EditOutlined, KeyOutlined } from '@ant-design/icons';
import type { KeychainItem } from '../types';
import { useAppTheme } from '../theme';

export const KeychainView: React.FC = () => {
  const { palette } = useAppTheme();
  const [items, setItems] = useState<KeychainItem[]>([]);
  const [isModalVisible, setIsModalVisible] = useState(false);
  const [editingItem, setEditingItem] = useState<KeychainItem | null>(null);
  const [form] = Form.useForm();

  useEffect(() => {
    fetchItems();
  }, []);

  const fetchItems = async () => {
    try {
      const response = await fetch('./api/keychain');
      const data = await response.json();
      setItems(data);
    } catch (err) {
      message.error('Failed to load keychain items');
    }
  };

  const handleSave = async (values: any) => {
    try {
      const url = editingItem ? `./api/keychain/${editingItem.id}` : './api/keychain';
      const method = editingItem ? 'PUT' : 'POST';
      
      await fetch(url, {
        method,
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ ...values, id: editingItem?.id || '' }),
      });
      
      message.success(editingItem ? 'Item updated' : 'Item added');
      setIsModalVisible(false);
      fetchItems();
    } catch (err) {
      message.error('Failed to save item');
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await fetch(`./api/keychain/${id}`, { method: 'DELETE' });
      message.success('Item deleted');
      fetchItems();
    } catch (err) {
      message.error('Failed to delete item');
    }
  };

  const columns = [
    {
      title: 'Name',
      dataIndex: 'name',
      key: 'name',
      render: (text: string) => <><KeyOutlined style={{ marginRight: 8, color: '#3b82f6' }} />{text}</>,
    },
    {
      title: 'Type',
      dataIndex: 'item_type',
      key: 'item_type',
      render: (text: string) => (text === 'Password' ? 'Password' : 'Private Key'),
    },
    {
      title: 'Actions',
      key: 'actions',
      render: (_: any, record: KeychainItem) => (
        <Space>
          <Button 
            type="text" 
            icon={<EditOutlined />} 
            style={{ color: '#3b82f6' }}
            onClick={() => {
              setEditingItem(record);
              form.setFieldsValue(record);
              setIsModalVisible(true);
            }}
          />
          <Popconfirm title="Delete this item?" onConfirm={() => handleDelete(record.id)}>
            <Button type="text" danger icon={<DeleteOutlined />} />
          </Popconfirm>
        </Space>
      ),
    },
  ];

  return (
    <div style={{ padding: 24, height: '100%', overflowY: 'auto', color: palette.text }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 24 }}>
        <div style={{ fontSize: 20, fontWeight: 500 }}>Keychain</div>
        <Button 
          type="primary" 
          icon={<PlusOutlined />}
          onClick={() => {
            setEditingItem(null);
            form.resetFields();
            setIsModalVisible(true);
          }}
        >
          Add Credential
        </Button>
      </div>

      <Table 
        dataSource={items} 
        columns={columns} 
        rowKey="id"
        pagination={false}
        style={{ background: palette.containerBg, borderRadius: 8 }}
      />

      <Modal
        title={editingItem ? "Edit Credential" : "New Credential"}
        open={isModalVisible}
        onCancel={() => setIsModalVisible(false)}
        footer={null}
        styles={{ body: { backgroundColor: palette.containerBg, color: palette.text }, header: { backgroundColor: palette.containerBg } }}
      >
        <Form form={form} layout="vertical" onFinish={handleSave}>
          <Form.Item name="name" label={<span style={{ color: palette.text }}>Name</span>} rules={[{ required: true }]}>
            <Input placeholder="e.g. My Production Key" style={{ backgroundColor: palette.inputBg, color: palette.text, borderColor: palette.border }} />
          </Form.Item>
          <Form.Item name="item_type" label={<span style={{ color: palette.text }}>Type</span>} rules={[{ required: true }]} initialValue="Password">
            <Select 
              dropdownStyle={{ backgroundColor: palette.containerBg, color: palette.text }}
              style={{ width: '100%' }}
            >
              <Select.Option value="Password">Password</Select.Option>
              <Select.Option value="PrivateKey">Private Key</Select.Option>
            </Select>
          </Form.Item>
          <Form.Item name="value" label={<span style={{ color: palette.text }}>Value (Password or PEM Content)</span>} rules={[{ required: true }]}>
            <Input.TextArea rows={4} style={{ backgroundColor: palette.inputBg, color: palette.text, borderColor: palette.border }} />
          </Form.Item>
          <div style={{ display: 'flex', justifyContent: 'flex-end', marginTop: 24 }}>
            <Button onClick={() => setIsModalVisible(false)} style={{ marginRight: 8, backgroundColor: 'transparent', color: palette.text }}>Cancel</Button>
            <Button type="primary" htmlType="submit">Save</Button>
          </div>
        </Form>
      </Modal>
    </div>
  );
};
