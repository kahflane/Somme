import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";

export type CursorStyle = "block" | "bar" | "underline";

export interface TerminalSettings {
  schemeId: string;
  fontId: string;
  fontSize: number;
  lineHeight: number;
  letterSpacing: number;
  cursorStyle: CursorStyle;
  cursorBlink: boolean;
  scrollback: number;
  brightBold: boolean;
  optionAsMeta: boolean;
  copyOnSelect: boolean;
  rightClickPaste: boolean;
}

interface TerminalSettingsState extends TerminalSettings {
  update: (patch: Partial<TerminalSettings>) => void;
  reset: () => void;
}

export const defaultTerminalSettings: TerminalSettings = {
  schemeId: "somme-dark",
  fontId: "jetbrains-mono",
  fontSize: 14,
  lineHeight: 1.2,
  letterSpacing: 0,
  cursorStyle: "block",
  cursorBlink: true,
  scrollback: 10000,
  brightBold: true,
  optionAsMeta: true,
  copyOnSelect: false,
  rightClickPaste: false,
};

export const useTerminalSettings = create<TerminalSettingsState>()(
  persist(
    (set) => ({
      ...defaultTerminalSettings,
      update: (patch) => set(patch),
      reset: () => set(defaultTerminalSettings),
    }),
    {
      name: "somme.terminal-settings",
      version: 1,
      storage: createJSONStorage(() => localStorage),
      partialize: ({ update: _update, reset: _reset, ...settings }) => settings,
    },
  ),
);
