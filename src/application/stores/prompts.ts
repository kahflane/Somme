import { create } from "zustand";
import type { PromptRequest } from "@/domain/types";

interface PromptsState {
  queue: PromptRequest[];
  push: (request: PromptRequest) => void;
  shift: () => void;
}

export const usePrompts = create<PromptsState>((set) => ({
  queue: [],
  push: (request) => set((state) => ({ queue: [...state.queue, request] })),
  shift: () => set((state) => ({ queue: state.queue.slice(1) })),
}));
