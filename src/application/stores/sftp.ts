import { create } from "zustand";
import type { Id } from "@/domain/types";

interface SftpState {
  remoteHostId: Id | null;
  nonce: number;
  connect: (hostId: Id) => void;
  disconnect: () => void;
}

export const useSftp = create<SftpState>((set) => ({
  remoteHostId: null,
  nonce: 0,
  connect: (hostId) => set((state) => ({ remoteHostId: hostId, nonce: state.nonce + 1 })),
  disconnect: () => set({ remoteHostId: null }),
}));
