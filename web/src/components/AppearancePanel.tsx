import { Segmented, Select, Slider, Typography } from 'antd';
import { FONT_FAMILIES, useAppTheme, type Mode } from '../theme';

const { Text } = Typography;

/** Terminal + app look, mirroring the palette tab in Termius' right sidebar. */
export function AppearancePanel() {
  const { palette, mode, setMode, appearance, setAppearance } = useAppTheme();

  const label = (s: string) => (
    <Text style={{ color: palette.textMuted, fontSize: 12, display: 'block', marginBottom: 6 }}>{s}</Text>
  );

  return (
    <div style={{ padding: 14, overflowY: 'auto', height: '100%' }}>
      <div style={{ marginBottom: 20 }}>
        {label('Interface theme')}
        <Segmented
          block
          value={mode}
          onChange={(v) => setMode(v as Mode)}
          options={[{ label: 'Light', value: 'light' }, { label: 'Dark', value: 'dark' }]}
        />
      </div>

      <div style={{ marginBottom: 20 }}>
        {label('Terminal font')}
        <Select
          value={appearance.fontFamily}
          onChange={(fontFamily) => setAppearance({ fontFamily })}
          options={FONT_FAMILIES}
          style={{ width: '100%' }}
        />
      </div>

      <div style={{ marginBottom: 20 }}>
        {label(`Font size — ${appearance.fontSize}px`)}
        <Slider
          min={10}
          max={22}
          value={appearance.fontSize}
          onChange={(fontSize) => setAppearance({ fontSize })}
        />
      </div>

      <div>
        {label('Preview')}
        <div style={{
          background: palette.terminalBg, borderRadius: 8, padding: 12,
          border: `1px solid ${palette.border}`,
          fontFamily: appearance.fontFamily, fontSize: appearance.fontSize,
          lineHeight: 1.5, color: '#e2e8f0', overflowX: 'auto',
        }}>
          <div><span style={{ color: '#22c55e' }}>user@host</span>:<span style={{ color: '#3b82f6' }}>~</span>$ ls -la</div>
          <div style={{ color: '#9ca3af' }}>drwxr-xr-x  4 user  staff  128 Sep  4 14:53 .</div>
        </div>
      </div>
    </div>
  );
}
