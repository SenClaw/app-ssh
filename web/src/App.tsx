import { useState, useEffect, useCallback, useMemo } from 'react';
import { ConfigProvider, Layout, Menu, Button, Row, Col, Empty, message, Input, Tabs, theme as antdTheme } from 'antd';
import { DesktopOutlined, KeyOutlined, SwapOutlined, CodeOutlined, PlusOutlined, HomeOutlined, FolderOutlined, SettingOutlined, LayoutOutlined } from '@ant-design/icons';
import type { Host, AppTab } from './types';
import { HostCard } from './components/HostCard';
import { HostDetails } from './components/HostDetails';
import { KeychainView } from './components/KeychainView';
import { PortForwardingView } from './components/PortForwardingView';
import { LogsView } from './components/LogsView';
import { SettingsModal } from './components/SettingsModal';
import { TerminalView } from './TerminalView';
import { SftpView } from './components/SftpView';
import { RightSidebar, type SidebarPanel } from './components/RightSidebar';
import {
  ThemeContext, PALETTES, detectInitialMode, detectInitialAppearance, useAppTheme,
  type Mode, type TerminalAppearance,
} from './theme';
import './App.css';

const { Header, Sider, Content } = Layout;

function App() {
  const [mode, setMode] = useState<Mode>(detectInitialMode);
  const [appearance, setAppearanceState] = useState<TerminalAppearance>(detectInitialAppearance);

  const setAppearance = useCallback((patch: Partial<TerminalAppearance>) => {
    setAppearanceState((prev) => {
      const next = { ...prev, ...patch };
      try { localStorage.setItem('ssh-appearance', JSON.stringify(next)); } catch { /* ignore */ }
      return next;
    });
  }, []);

  // Follow senclaw's dark/light mode (same handshake deepwiki uses).
  useEffect(() => {
    const onMessage = (e: MessageEvent) => {
      const d = e.data;
      if (!d || typeof d !== 'object') return;
      const t = d.theme ?? d.env?.theme;
      if ((d.type === 'senclaw:init' || d.type === 'senclaw:theme') && (t === 'dark' || t === 'light')) {
        setMode(t);
      }
    };
    window.addEventListener('message', onMessage);
    try { window.parent?.postMessage({ type: 'senclaw:ready' }, '*'); } catch { /* ignore */ }
    return () => window.removeEventListener('message', onMessage);
  }, []);

  useEffect(() => {
    try { localStorage.setItem('ssh-mode', mode); } catch { /* ignore */ }
  }, [mode]);

  const isDark = mode === 'dark';
  const palette = PALETTES[mode];

  // Expose the palette to CSS (App.css / index.css use these variables).
  useEffect(() => {
    const root = document.documentElement;
    root.style.setProperty('--app-layout-bg', palette.layoutBg);
    root.style.setProperty('--app-container-bg', palette.containerBg);
    root.style.setProperty('--app-elevated', palette.elevated);
    root.style.setProperty('--app-border', palette.border);
    root.style.setProperty('--app-text', palette.text);
    root.style.setProperty('--app-text-muted', palette.textMuted);
    document.body.style.backgroundColor = palette.layoutBg;
  }, [palette]);

  return (
    <ConfigProvider
      theme={{
        algorithm: isDark ? antdTheme.darkAlgorithm : antdTheme.defaultAlgorithm,
        token: {
          colorPrimary: '#3b82f6',
          colorBgBase: palette.layoutBg,
          colorTextBase: palette.text,
          colorBorder: palette.border,
          colorBgContainer: palette.containerBg,
          colorBgLayout: palette.layoutBg,
          borderRadius: 8,
        },
        components: {
          Layout: {
            siderBg: palette.containerBg,
            headerBg: palette.containerBg,
            bodyBg: palette.layoutBg,
          },
          Menu: {
            itemBg: palette.containerBg,
            itemColor: palette.textMuted,
            itemSelectedBg: palette.elevated,
            itemSelectedColor: palette.text,
          },
          Card: {
            colorBgContainer: palette.containerBg,
          },
          Tabs: {
            itemColor: palette.textMuted,
            itemHoverColor: palette.text,
            itemSelectedColor: palette.text,
            cardBg: palette.layoutBg,
          },
        },
      }}
    >
      <ThemeContext.Provider value={{ mode, isDark, palette, setMode, appearance, setAppearance }}>
        <Shell />
      </ThemeContext.Provider>
    </ConfigProvider>
  );
}

function Shell() {
  const { palette } = useAppTheme();
  const [hosts, setHosts] = useState<Host[]>([]);
  const [selectedHost, setSelectedHost] = useState<Host | null>(null);
  const [isEditing, setIsEditing] = useState(false);
  
  const [tabs, setTabs] = useState<AppTab[]>(() => {
    const saved = localStorage.getItem('ssh-tabs');
    if (saved) {
      try {
        return JSON.parse(saved);
      } catch (e) {}
    }
    return [
      { id: 'home', type: 'home', title: 'Vaults' },
      { id: 'sftp', type: 'sftp', title: 'SFTP' }
    ];
  });
  const [activeTabId, setActiveTabId] = useState<string>('home');
  const [currentMenu, setCurrentMenu] = useState<string>('hosts');
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(() => {
    try { return localStorage.getItem('ssh-sidebar-open') !== 'false'; } catch { return true; }
  });
  const [sidebarPanel, setSidebarPanel] = useState<SidebarPanel>('snippets');

  useEffect(() => {
    try { localStorage.setItem('ssh-sidebar-open', String(sidebarOpen)); } catch { /* ignore */ }
  }, [sidebarOpen]);

  // Snippets and history send into whichever terminal tab is in front; on
  // Vaults/SFTP there is nothing to send to and the panels say so.
  const activeTerminal = useMemo(() => {
    const tab = tabs.find((t) => t.id === activeTabId && t.type === 'terminal');
    return tab ? { tabId: tab.id, label: tab.title } : null;
  }, [tabs, activeTabId]);

  useEffect(() => {
    localStorage.setItem('ssh-tabs', JSON.stringify(tabs));
  }, [tabs]);

  useEffect(() => {
    fetchHosts();

    const evtSource = new EventSource("./api/ui-events");
    evtSource.addEventListener("ui-event", (e) => {
      try {
        const data = JSON.parse(e.data);
        if (data.type === "mcp_connect") {
          setTabs(prev => {
            const tabExists = prev.find(t => t.type === 'terminal' && t.host?.id === data.host_id);
            if (!tabExists) {
              const tabId = `term-${data.host_id}-${Date.now()}`;
              const newTabs = [...prev, { id: tabId, type: 'terminal' as const, title: data.host.name || data.host.host, host: data.host }];
              setActiveTabId(tabId);
              return newTabs;
            }
            return prev;
          });
        } else if (data.type === "mcp_execute") {
           window.dispatchEvent(new CustomEvent('mcp-log', { detail: data }));
        }
      } catch (err) {}
    });
    return () => evtSource.close();
  }, []);

  const fetchHosts = async () => {
    try {
      const response = await fetch('./api/hosts');
      const data = await response.json();
      setHosts(data);
    } catch (err) {
      console.error('Failed to fetch hosts', err);
      message.error('Failed to load hosts');
    }
  };

  const handleSaveHost = async (host: Host) => {
    try {
      if (host.id) {
        await fetch(`./api/hosts/${host.id}`, {
          method: 'PUT',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(host),
        });
        message.success('Host updated');
      } else {
        await fetch('./api/hosts', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(host),
        });
        message.success('Host added');
      }
      fetchHosts();
      setIsEditing(false);
      setSelectedHost(null);
    } catch (err) {
      message.error('Failed to save host');
    }
  };

  const handleDeleteHost = async (id: string) => {
    try {
      await fetch(`./api/hosts/${id}`, { method: 'DELETE' });
      message.success('Host deleted');
      fetchHosts();
      setIsEditing(false);
      setSelectedHost(null);
    } catch (err) {
      message.error('Failed to delete host');
    }
  };

  const handleConnect = (host: Host) => {
    const tabId = `term-${host.id}-${Date.now()}`;
    setTabs([...tabs, { id: tabId, type: 'terminal', title: host.name || host.host, host }]);
    setActiveTabId(tabId);
  };

  const removeTab = (targetKey: string) => {
    const newTabs = tabs.filter(tab => tab.id !== targetKey);
    setTabs(newTabs);
    if (activeTabId === targetKey) {
      // Prefer the most recent remaining terminal tab; if none, fall back to home.
      const lastTerminal = [...newTabs].reverse().find(t => t.type === 'terminal');
      setActiveTabId(lastTerminal ? lastTerminal.id : 'home');
    }
  };

  const onEditTab = (targetKey: any, action: 'add' | 'remove') => {
    if (action === 'remove') {
      removeTab(targetKey as string);
    }
  };

  const renderHomeContent = () => {
    switch (currentMenu) {
      case 'hosts':
        return (
          <Layout style={{ height: '100%', backgroundColor: palette.layoutBg }}>
            <Header style={{ padding: '0 24px', display: 'flex', alignItems: 'center', borderBottom: `1px solid ${palette.border}`, backgroundColor: palette.layoutBg }}>
              <Input.Search 
                placeholder="Find a host or ssh user@hostname..." 
                style={{ maxWidth: 400 }} 
                className="custom-search"
              />
              <Button 
                type="primary" 
                icon={<PlusOutlined />} 
                style={{ marginLeft: 'auto' }}
                onClick={() => {
                  setSelectedHost(null);
                  setIsEditing(true);
                }}
              >
                New Host
              </Button>
            </Header>
            <Content style={{ padding: '24px', overflowY: 'auto' }}>
              <div style={{ marginBottom: 24, color: palette.textMuted, fontSize: 16 }}>
                Hosts ({hosts.length})
              </div>
              {hosts.length === 0 ? (
                <Empty description={<span style={{ color: palette.textMuted }}>No hosts found. Add one to get started.</span>} />
              ) : (
                <Row gutter={[16, 16]}>
                  {hosts.map(host => (
                    <Col xs={24} sm={12} md={8} lg={6} key={host.id}>
                      <HostCard
                        host={host}
                        selected={selectedHost?.id === host.id}
                        onClick={(h) => {
                          setSelectedHost(h);
                          setIsEditing(true);
                        }}
                        onDoubleClick={(h) => handleConnect(h)}
                      />
                    </Col>
                  ))}
                </Row>
              )}
            </Content>
          </Layout>
        );
      case 'keychain':
        return <KeychainView />;
      case 'port-forwarding':
        return <PortForwardingView />;
      case 'logs':
        return <LogsView />;
      default:
        return null;
    }
  };

  return (
    <>
      <div style={{ display: 'flex', flexDirection: 'column', height: '100vh', background: palette.layoutBg }}>
        {/* Top Tab Bar - Termius Style */}
        <div className="termius-tab-bar" style={{
          background: palette.layoutBg,
          borderBottom: `1px solid ${palette.border}`,
          paddingTop: 8,
          paddingLeft: 8,
          paddingRight: 8,
          display: 'flex',
          alignItems: 'center',
        }}>
          <Button
            type="text"
            icon={<SettingOutlined />}
            onClick={() => setSettingsOpen(true)}
            title="Settings"
            style={{ color: palette.textMuted, marginRight: 8, alignSelf: 'center' }}
          />
          <Tabs
            hideAdd
            type="editable-card"
            onChange={(key) => setActiveTabId(key)}
            activeKey={activeTabId}
            onEdit={onEditTab}
            items={tabs.map(tab => ({
              label: (
                <span style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                  {tab.type === 'home' ? <HomeOutlined /> : tab.type === 'sftp' ? <FolderOutlined /> : <DesktopOutlined />}
                  {tab.title}
                </span>
              ),
              key: tab.id,
              closable: tab.type !== 'home' && tab.type !== 'sftp',
            }))}
            style={{ marginBottom: -1, flex: 1 }} // Hide bottom border, expand
          />
          <Button
            type="text"
            icon={<LayoutOutlined />}
            onClick={() => setSidebarOpen((v) => !v)}
            title={sidebarOpen ? 'Hide sidebar' : 'Show sidebar'}
            style={{ color: sidebarOpen ? '#22c55e' : palette.textMuted, marginLeft: 8, alignSelf: 'center' }}
          />
        </div>

        {/* Main Area */}
        <div style={{ flex: 1, display: 'flex', overflow: 'hidden' }}>
          {/* Sidebar */}
          <div style={{ display: activeTabId === 'home' ? 'block' : 'none' }}>
            <Sider width={250} style={{ borderRight: `1px solid ${palette.border}`, background: palette.containerBg, height: '100%' }}>
            <Menu
              mode="inline"
              selectedKeys={[activeTabId === 'home' ? currentMenu : '']}
              style={{ borderRight: 0, marginTop: 16, background: 'transparent' }}
              onClick={({ key }) => {
                if (key !== 'terminal') {
                  setCurrentMenu(key);
                  setActiveTabId('home');
                }
              }}
              items={[
                { key: 'hosts', icon: <DesktopOutlined />, label: 'Hosts' },
                { key: 'keychain', icon: <KeyOutlined />, label: 'Keychain' },
                { key: 'port-forwarding', icon: <SwapOutlined />, label: 'Port Forwarding' },
                { key: 'logs', icon: <CodeOutlined />, label: 'Logs' },
              ]}
            />
            </Sider>
          </div>

          {/* Content Area */}
          <div style={{ flex: 1, display: 'flex', overflow: 'hidden', position: 'relative' }}>
            
            {/* Home Views */}
            <div style={{ 
              display: activeTabId === 'home' ? 'flex' : 'none', 
              flex: 1, 
              width: '100%',
              height: '100%' 
            }}>
              <Layout style={{ height: '100%', background: palette.layoutBg }}>
                {renderHomeContent()}
              </Layout>
              {isEditing && currentMenu === 'hosts' && activeTabId === 'home' && (
                <Sider width={350} style={{ borderLeft: `1px solid ${palette.border}`, background: palette.containerBg }}>
                  <HostDetails
                    host={selectedHost}
                    onSave={handleSaveHost}
                    onDelete={handleDeleteHost}
                    onConnect={handleConnect}
                    onClose={() => {
                      setIsEditing(false);
                      setSelectedHost(null);
                    }}
                  />
                </Sider>
              )}
            </div>

            {/* SFTP View */}
            <div style={{ 
              display: activeTabId === 'sftp' ? 'block' : 'none', 
              flex: 1, 
              width: '100%',
              height: '100%' 
            }}>
              <SftpView hosts={hosts} />
            </div>

            {/* Terminal Views */}
            {tabs.filter(t => t.type === 'terminal').map(tab => (
              <div 
                key={tab.id}
                style={{ 
                  display: activeTabId === tab.id ? 'block' : 'none', 
                  width: '100%', 
                  height: '100%',
                  flex: 1,
                  background: palette.terminalBg
                }}
              >
                {tab.host && <TerminalView host={tab.host} isActive={activeTabId === tab.id} tabId={tab.id} />}
              </div>
            ))}

          </div>

          {sidebarOpen && (
            <RightSidebar
              panel={sidebarPanel}
              onPanelChange={setSidebarPanel}
              hosts={hosts}
              onConnect={handleConnect}
              activeTerminal={activeTerminal}
            />
          )}
        </div>
      </div>
      <SettingsModal open={settingsOpen} onClose={() => setSettingsOpen(false)} />
    </>
  );
}

export default App;
