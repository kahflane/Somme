import { create } from "zustand";

const MAX_COMMANDS = 200;

interface ShellHistoryState {
  commands: string[];
  record: (command: string) => void;
  clear: () => void;
}

// Kept in memory only: typed commands can hold secrets, and the vault is the
// only place Somme stores anything sensitive.
export const useShellHistory = create<ShellHistoryState>((set) => ({
  commands: [],
  record: (command) =>
    set((state) => ({
      commands: [command, ...state.commands.filter((item) => item !== command)].slice(0, MAX_COMMANDS),
    })),
  clear: () => set({ commands: [] }),
}));
