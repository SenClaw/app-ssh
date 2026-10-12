import React, { useRef, useEffect } from 'react';
import { Card, Space } from 'antd';
import { DesktopOutlined } from '@ant-design/icons';
import type { Host } from '../types';
import { useAppTheme } from '../theme';

interface HostCardProps {
  host: Host;
  selected: boolean;
  onClick: (host: Host) => void;
  onDoubleClick?: (host: Host) => void;
}

const DBLCLICK_DELAY_MS = 250;

export const HostCard: React.FC<HostCardProps> = ({ host, selected, onClick, onDoubleClick }) => {
  const { palette } = useAppTheme();
  const clickTimer = useRef<number | null>(null);

  useEffect(() => () => {
    if (clickTimer.current !== null) {
      window.clearTimeout(clickTimer.current);
    }
  }, []);

  const handleClick = () => {
    if (clickTimer.current !== null) return;
    clickTimer.current = window.setTimeout(() => {
      clickTimer.current = null;
      onClick(host);
    }, DBLCLICK_DELAY_MS);
  };

  const handleDoubleClick = () => {
    if (clickTimer.current !== null) {
      window.clearTimeout(clickTimer.current);
      clickTimer.current = null;
    }
    onDoubleClick?.(host);
  };

  return (
    <Card
      hoverable
      onClick={handleClick}
      onDoubleClick={handleDoubleClick}
      style={{
        backgroundColor: selected ? palette.elevated : palette.containerBg,
        borderColor: selected ? '#3b82f6' : palette.border,
        cursor: 'pointer',
        borderRadius: 8,
      }}
      bodyStyle={{ padding: '16px' }}
    >
      <Space align="start">
        <div style={{
          backgroundColor: '#f97316',
          borderRadius: '50%',
          width: 32,
          height: 32,
          display: 'flex',
          justifyContent: 'center',
          alignItems: 'center',
          color: '#fff',
          marginTop: 2
        }}>
          <DesktopOutlined />
        </div>
        <div>
          <div style={{ color: palette.text, fontWeight: 600, fontSize: 16 }}>
            {host.name || host.host}
          </div>
          <div style={{ color: palette.textMuted, fontSize: 12 }}>
            ssh, {host.user}
          </div>
        </div>
      </Space>
    </Card>
  );
};
