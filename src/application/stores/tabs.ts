import { create } from "zustand";
import { useSftp } from "@/application/stores/sftp";
import type { Id } from "@/domain/types";

export const VAULT_TAB_ID = "vault";
export const SFTP_TAB_ID = "sftp";

export type WorkTabKind = "ssh" | "local";

export interface WorkTab {
  id: string;
  kind: WorkTabKind;
  title: string;
  hostId: Id | null;
  nonce: number;
}

interface HostRef {
  id: Id;
  label: string;
}

interface TabsState {
  tabs: WorkTab[];
  activeId: string;
  lastTerminalId: string | null;
  sessions: Record<string, Id>;
  broadcast: boolean;
  openSsh: (host: HostRef) => void;
  openLocal: () => void;
  openSftp: (host: HostRef) => void;
  activate: (id: string) => void;
  close: (id: string) => void;
  reconnect: (id: string) => void;
  bindSession: (tabId: string, sessionId: Id | null) => void;
  toggleBroadcast: () => void;
  reset: () => void;
}

function newTab(kind: WorkTabKind, title: string, hostId: Id | null): WorkTab {
  return { id: crypto.randomUUID(), kind, title, hostId, nonce: 0 };
}

export const useTabs = create<TabsState>((set) => ({
  tabs: [],
  activeId: VAULT_TAB_ID,
  lastTerminalId: null,
  sessions: {},
  broadcast: false,
  openSsh: (host) => {
    const tab = newTab("ssh", host.label, host.id);
    set((state) => ({ tabs: [...state.tabs, tab], activeId: tab.id, lastTerminalId: tab.id }));
  },
  openLocal: () => {
    const tab = newTab("local", "Local terminal", null);
    set((state) => ({ tabs: [...state.tabs, tab], activeId: tab.id, lastTerminalId: tab.id }));
  },
  openSftp: (host) => {
    useSftp.getState().connect(host.id);
    set({ activeId: SFTP_TAB_ID });
  },
  activate: (id) =>
    set((state) => ({
      activeId: id,
      lastTerminalId: state.tabs.some((tab) => tab.id === id) ? id : state.lastTerminalId,
    })),
  close: (id) =>
    set((state) => {
      const index = state.tabs.findIndex((tab) => tab.id === id);
      const tabs = state.tabs.filter((tab) => tab.id !== id);
      const sessions = { ...state.sessions };
      delete sessions[id];
      const fallback = tabs[Math.min(index, tabs.length - 1)]?.id ?? VAULT_TAB_ID;
      const activeId = state.activeId === id ? fallback : state.activeId;
      const lastTerminalId =
        state.lastTerminalId === id
          ? tabs.some((tab) => tab.id === activeId)
            ? activeId
            : null
          : state.lastTerminalId;
      return { tabs, sessions, activeId, lastTerminalId };
    }),
  reconnect: (id) =>
    set((state) => ({
      tabs: state.tabs.map((tab) => (tab.id === id ? { ...tab, nonce: tab.nonce + 1 } : tab)),
    })),
  bindSession: (tabId, sessionId) =>
    set((state) => {
      const sessions = { ...state.sessions };
      if (sessionId) sessions[tabId] = sessionId;
      else delete sessions[tabId];
      return { sessions };
    }),
  toggleBroadcast: () => set((state) => ({ broadcast: !state.broadcast })),
  reset: () => {
    useSftp.getState().disconnect();
    set({ tabs: [], activeId: VAULT_TAB_ID, lastTerminalId: null, sessions: {}, broadcast: false });
  },
}));
