import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";
import type { Id } from "@/domain/types";

const MAX_ENTRIES = 500;

export interface ConnectionLog {
  id: string;
  hostId: Id;
  label: string;
  address: string;
  port: number;
  username: string | null;
  startedAt: number;
  endedAt: number | null;
}

export type ConnectionStart = Omit<ConnectionLog, "id" | "startedAt" | "endedAt">;

interface ConnectionLogsState {
  entries: ConnectionLog[];
  start: (connection: ConnectionStart) => string;
  finish: (id: string) => void;
  remove: (id: string) => void;
  clear: () => void;
}

export const useConnectionLogs = create<ConnectionLogsState>()(
  persist(
    (set) => ({
      entries: [],
      start: (connection) => {
        const entry: ConnectionLog = {
          ...connection,
          id: crypto.randomUUID(),
          startedAt: Date.now(),
          endedAt: null,
        };
        set((state) => ({ entries: [entry, ...state.entries].slice(0, MAX_ENTRIES) }));
        return entry.id;
      },
      finish: (id) =>
        set((state) => ({
          entries: state.entries.map((entry) =>
            entry.id === id && entry.endedAt === null ? { ...entry, endedAt: Date.now() } : entry,
          ),
        })),
      remove: (id) => set((state) => ({ entries: state.entries.filter((entry) => entry.id !== id) })),
      clear: () => set({ entries: [] }),
    }),
    {
      name: "somme.connection-logs",
      version: 1,
      storage: createJSONStorage(() => localStorage),
      partialize: ({ entries }) => ({ entries }),
    },
  ),
);
