import React, { useState, useEffect } from 'react';
import { Layout, Table, Button, Input, Row, Col, Typography, Space, Dropdown, message } from 'antd';
import type { MenuProps } from 'antd';
import { FolderOutlined, FileOutlined, DesktopOutlined, ArrowLeftOutlined, CloudServerOutlined, CloudUploadOutlined } from '@ant-design/icons';
import type { Host, FileNode } from '../types';
import { useAppTheme } from '../theme';

const { Header, Content } = Layout;
const { Text } = Typography;

interface SftpViewProps {
  hosts: Host[];
}

/** Append a name to a remote directory path. The remote pane starts at `.`,
 *  the login directory, where a bare name is already the path the server wants. */
const joinRemote = (base: string, name: string): string => {
  if (!base || base === '.') return name;
  return base.endsWith('/') ? `${base}${name}` : `${base}/${name}`;
};

export const SftpView: React.FC<SftpViewProps> = ({ hosts }) => {
  const { palette } = useAppTheme();
  const [localPath, setLocalPath] = useState('/');
  const [localFiles, setLocalFiles] = useState<FileNode[]>([]);
  const [remotePath, setRemotePath] = useState('/');
  const [remoteFiles, setRemoteFiles] = useState<FileNode[]>([]);
  
  const [connId, setConnId] = useState<string | null>(null);
  const [selectedHost, setSelectedHost] = useState<Host | null>(null);
  const [loadingLocal, setLoadingLocal] = useState(false);
  const [loadingRemote, setLoadingRemote] = useState(false);
  const [uploading, setUploading] = useState(false);

  useEffect(() => {
    fetchLocalFiles(localPath);
  }, [localPath]);

  useEffect(() => {
    if (connId) {
      fetchRemoteFiles(connId, remotePath);
    }
  }, [connId, remotePath]);

  const fetchLocalFiles = async (path: string) => {
    setLoadingLocal(true);
    try {
      const res = await fetch(`./api/sftp/local/ls?path=${encodeURIComponent(path)}`);
      if (res.ok) {
        const data = await res.json();
        // Sort directories first
        data.sort((a: FileNode, b: FileNode) => {
          if (a.is_dir === b.is_dir) return a.name.localeCompare(b.name);
          return a.is_dir ? -1 : 1;
        });
        setLocalFiles(data);
      } else {
        message.error('Failed to load local files');
      }
    } catch (err) {
      message.error('Failed to load local files');
    } finally {
      setLoadingLocal(false);
    }
  };

  const fetchRemoteFiles = async (id: string, path: string) => {
    setLoadingRemote(true);
    try {
      const res = await fetch(`./api/sftp/remote/${id}/ls?path=${encodeURIComponent(path)}`);
      if (res.ok) {
        const data = await res.json();
        data.sort((a: FileNode, b: FileNode) => {
          if (a.is_dir === b.is_dir) return a.name.localeCompare(b.name);
          return a.is_dir ? -1 : 1;
        });
        setRemoteFiles(data);
      } else {
        message.error('Failed to load remote files');
      }
    } catch (err) {
      message.error('Failed to load remote files');
    } finally {
      setLoadingRemote(false);
    }
  };

  const handleConnect = async (host: Host) => {
    try {
      setLoadingRemote(true);
      const res = await fetch('./api/sftp/connect', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ host_id: host.id })
      });
      if (res.ok) {
        const data = await res.json();
        setConnId(data.conn_id);
        setSelectedHost(host);
        setRemotePath('.'); // Start at home directory for remote
      } else {
        message.error('Failed to connect via SFTP');
      }
    } catch (err) {
      message.error('SFTP connection error');
    } finally {
      setLoadingRemote(false);
    }
  };

  /** Right-click upload: a folder goes up whole, a file on its own. The
   *  destination is whatever directory the remote pane is showing. */
  const handleUpload = async (record: FileNode) => {
    if (!connId) {
      message.warning('Connect to a host first');
      return;
    }

    const target = joinRemote(remotePath, record.name);
    const endpoint = record.is_dir ? 'upload-dir' : 'upload';
    const key = 'sftp-upload';

    setUploading(true);
    message.open({ key, type: 'loading', content: `Uploading ${record.name}...`, duration: 0 });

    try {
      const res = await fetch(`./api/sftp/remote/${connId}/${endpoint}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ source: record.path, target })
      });

      if (!res.ok) {
        throw new Error((await res.text()) || `HTTP ${res.status}`);
      }

      if (record.is_dir) {
        const report = await res.json();
        const summary = `${record.name}: ${report.files} files in ${report.dirs} folders`;
        if (report.failed > 0) {
          // Partial success is still success for the rest of the tree, so say
          // both halves rather than reporting a flat failure.
          message.open({ key, type: 'warning', content: `${summary}, ${report.failed} failed`, duration: 6 });
          report.errors.forEach((e: string) => console.warn('[sftp upload]', e));
        } else {
          message.open({ key, type: 'success', content: `Uploaded ${summary}`, duration: 3 });
        }
      } else {
        message.open({ key, type: 'success', content: `Uploaded ${record.name}`, duration: 3 });
      }

      fetchRemoteFiles(connId, remotePath);
    } catch (err) {
      message.open({ key, type: 'error', content: `Upload failed: ${err}`, duration: 6 });
    } finally {
      setUploading(false);
    }
  };

  const localRowMenu = (record: FileNode): MenuProps => ({
    items: [
      {
        key: 'upload',
        icon: <CloudUploadOutlined />,
        label: record.is_dir ? 'Upload folder to remote' : 'Upload to remote',
        disabled: !connId || uploading
      }
    ],
    onClick: ({ key }) => {
      if (key === 'upload') handleUpload(record);
    }
  });

  const renderFileTable = (
    files: FileNode[], 
    loading: boolean, 
    currentPath: string, 
    onNavigate: (path: string) => void,
    rowMenu?: (record: FileNode) => MenuProps
  ) => {
    const columns = [
      {
        title: 'Name',
        dataIndex: 'name',
        key: 'name',
        render: (text: string, record: FileNode) => {
          const cell = (
            <Space style={{ cursor: record.is_dir ? 'pointer' : 'default' }} onClick={() => {
              if (record.is_dir) {
                const separator = currentPath.endsWith('/') ? '' : '/';
                onNavigate(currentPath + separator + text);
              }
            }}>
              {record.is_dir ? <FolderOutlined style={{ color: '#3b82f6' }} /> : <FileOutlined />}
              {text}
            </Space>
          );

          if (!rowMenu) return cell;

          // A block wrapper so the right-click target is the whole name cell,
          // not just the text the name happens to fill.
          return (
            <Dropdown menu={rowMenu(record)} trigger={['contextMenu']}>
              <div>{cell}</div>
            </Dropdown>
          );
        }
      },
      {
        title: 'Date Modified',
        dataIndex: 'modified_time',
        key: 'modified_time',
        width: 150,
        render: (time: number) => new Date(time * 1000).toLocaleDateString()
      },
      {
        title: 'Size',
        dataIndex: 'size',
        key: 'size',
        width: 100,
        render: (size: number, record: FileNode) => record.is_dir ? '--' : `${(size / 1024).toFixed(1)} KB`
      }
    ];

    return (
      <Table 
        dataSource={files} 
        columns={columns} 
        rowKey="name"
        loading={loading}
        size="small"
        pagination={false}
        scroll={{ y: 'calc(100vh - 200px)' }}
        className="sftp-table"
      />
    );
  };

  return (
    <div style={{ display: 'flex', width: '100%', height: '100%', background: palette.layoutBg }}>
      {/* Local Pane */}
      <div style={{ flex: 1, borderRight: `1px solid ${palette.border}`, display: 'flex', flexDirection: 'column' }}>
        <Header style={{ background: palette.containerBg, padding: '0 16px', display: 'flex', alignItems: 'center', borderBottom: `1px solid ${palette.border}`, height: 48 }}>
          <Space>
            <DesktopOutlined style={{ color: '#3b82f6' }} />
            <Text style={{ color: palette.text, fontWeight: 600 }}>Local</Text>
          </Space>
        </Header>
        <div style={{ padding: '8px 16px', background: palette.layoutBg, borderBottom: `1px solid ${palette.border}`, display: 'flex', gap: 8 }}>
          <Button icon={<ArrowLeftOutlined />} size="small" onClick={() => {
            const parts = localPath.split('/').filter(Boolean);
            parts.pop();
            setLocalPath('/' + parts.join('/'));
          }} />
          <Input size="small" value={localPath} onChange={e => setLocalPath(e.target.value)} onPressEnter={() => fetchLocalFiles(localPath)} style={{ flex: 1 }} />
        </div>
        <Content style={{ overflow: 'hidden' }}>
          {renderFileTable(localFiles, loadingLocal, localPath, setLocalPath, localRowMenu)}
        </Content>
      </div>

      {/* Remote Pane */}
      <div style={{ flex: 1, display: 'flex', flexDirection: 'column' }}>
        <Header style={{ background: palette.containerBg, padding: '0 16px', display: 'flex', alignItems: 'center', borderBottom: `1px solid ${palette.border}`, height: 48 }}>
          <Space>
            <CloudServerOutlined style={{ color: '#f59e0b' }} />
            <Text style={{ color: palette.text, fontWeight: 600 }}>
              {selectedHost ? selectedHost.name || selectedHost.host : 'Remote'}
            </Text>
          </Space>
          {connId && (
            <Button size="small" type="text" danger style={{ marginLeft: 'auto' }} onClick={() => {
              setConnId(null);
              setSelectedHost(null);
              setRemoteFiles([]);
            }}>
              Disconnect
            </Button>
          )}
        </Header>

        {!connId ? (
          <Content style={{ padding: 24, overflowY: 'auto' }}>
            <div style={{ marginBottom: 16, color: palette.textMuted }}>Select a host to connect</div>
            <Row gutter={[16, 16]}>
              {hosts.map(host => (
                <Col span={24} key={host.id}>
                  <div 
                    onClick={() => handleConnect(host)}
                    style={{ 
                      padding: 12, 
                      background: palette.containerBg, 
                      borderRadius: 8, 
                      cursor: 'pointer',
                      border: `1px solid ${palette.border}`,
                      display: 'flex',
                      alignItems: 'center',
                      gap: 12
                    }}
                    className="host-hover"
                  >
                    <div style={{ background: '#f59e0b', padding: 8, borderRadius: '50%', color: palette.text }}>
                      <CloudServerOutlined />
                    </div>
                    <div>
                      <div style={{ color: palette.text, fontWeight: 500 }}>{host.name || host.host}</div>
                      <div style={{ color: palette.textMuted, fontSize: 12 }}>{host.user}@{host.host}:{host.port}</div>
                    </div>
                  </div>
                </Col>
              ))}
            </Row>
          </Content>
        ) : (
          <>
            <div style={{ padding: '8px 16px', background: palette.layoutBg, borderBottom: `1px solid ${palette.border}`, display: 'flex', gap: 8 }}>
              <Button icon={<ArrowLeftOutlined />} size="small" onClick={() => {
                const parts = remotePath.split('/').filter(Boolean);
                parts.pop();
                setRemotePath('/' + parts.join('/'));
              }} />
              <Input size="small" value={remotePath} onChange={e => setRemotePath(e.target.value)} onPressEnter={() => fetchRemoteFiles(connId, remotePath)} style={{ flex: 1 }} />
            </div>
            <Content style={{ overflow: 'hidden' }}>
              {renderFileTable(remoteFiles, loadingRemote, remotePath, setRemotePath)}
            </Content>
          </>
        )}
      </div>
    </div>
  );
};
