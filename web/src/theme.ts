import { createContext, useContext } from 'react';

/** Dark / light mode. Mirrors the contract deepwiki uses (see apps/deepwiki). */
export type Mode = 'dark' | 'light';

/** Semantic colors used across the SSH Manager UI, resolved per mode. */
export interface Palette {
  /** Outermost / content background (was #111827). */
  layoutBg: string;
  /** Sider / header / card / modal surface (was #1f2937). */
  containerBg: string;
  /** Form input background (was #111827). */
  inputBg: string;
  /** Hover / selected / active-tab surface (was #374151). */
  elevated: string;
  /** Hairline borders (was #374151). */
  border: string;
  /** Primary text (was #fff / #f9fafb). */
  text: string;
  /** Muted / secondary text (was #9ca3af). */
  textMuted: string;
  /** Embedded terminal background — kept dark in both modes (terminals read better dark). */
  terminalBg: string;
  /** Logs console background — kept dark in both modes (colored log text needs a dark surface). */
  consoleBg: string;
  /** Logs console default text. */
  consoleText: string;
}

export const PALETTES: Record<Mode, Palette> = {
  dark: {
    layoutBg: '#111827',
    containerBg: '#1f2937',
    inputBg: '#111827',
    elevated: '#374151',
    border: '#374151',
    text: '#f9fafb',
    textMuted: '#9ca3af',
    terminalBg: '#0f172a',
    consoleBg: '#0b1220',
    consoleText: '#e5e7eb',
  },
  light: {
    layoutBg: '#f3f4f6',
    containerBg: '#ffffff',
    inputBg: '#ffffff',
    elevated: '#eef2f7',
    border: '#e5e7eb',
    text: '#111827',
    textMuted: '#6b7280',
    terminalBg: '#0f172a',
    consoleBg: '#0b1220',
    consoleText: '#e5e7eb',
  },
};

/** Resolve the initial mode: saved preference → system preference → dark. */
export function detectInitialMode(): Mode {
  try {
    const saved = localStorage.getItem('ssh-mode');
    if (saved === 'dark' || saved === 'light') return saved;
  } catch { /* ignore */ }
  if (typeof window !== 'undefined' && window.matchMedia?.('(prefers-color-scheme: dark)').matches) {
    return 'dark';
  }
  return 'light';
}

/** Terminal typography, tweakable from the sidebar's appearance panel. */
export interface TerminalAppearance {
  fontSize: number;
  fontFamily: string;
}

export const FONT_FAMILIES = [
  { value: 'Menlo, Monaco, "Courier New", monospace', label: 'Menlo' },
  { value: '"SF Mono", Menlo, monospace', label: 'SF Mono' },
  { value: '"JetBrains Mono", Menlo, monospace', label: 'JetBrains Mono' },
  { value: '"Fira Code", Menlo, monospace', label: 'Fira Code' },
  { value: 'Consolas, "Courier New", monospace', label: 'Consolas' },
];

export const DEFAULT_APPEARANCE: TerminalAppearance = {
  fontSize: 14,
  fontFamily: FONT_FAMILIES[0].value,
};

/** Resolve the saved terminal appearance, falling back to the defaults. */
export function detectInitialAppearance(): TerminalAppearance {
  try {
    const raw = localStorage.getItem('ssh-appearance');
    if (raw) return { ...DEFAULT_APPEARANCE, ...JSON.parse(raw) };
  } catch { /* ignore */ }
  return DEFAULT_APPEARANCE;
}

export interface ThemeCtx {
  mode: Mode;
  isDark: boolean;
  palette: Palette;
  setMode: (mode: Mode) => void;
  appearance: TerminalAppearance;
  setAppearance: (patch: Partial<TerminalAppearance>) => void;
}

export const ThemeContext = createContext<ThemeCtx>({
  mode: 'dark',
  isDark: true,
  palette: PALETTES.dark,
  setMode: () => {},
  appearance: DEFAULT_APPEARANCE,
  setAppearance: () => {},
});

/** Access the active mode + resolved palette from any component. */
export const useAppTheme = () => useContext(ThemeContext);
