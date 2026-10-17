import { create } from "zustand";

export type Section = "hosts" | "keychain" | "forwards" | "snippets" | "knownHosts" | "logs" | "settings";

interface NavigationState {
  section: Section;
  setSection: (section: Section) => void;
}

export const useNavigation = create<NavigationState>((set) => ({
  section: "hosts",
  setSection: (section) => set({ section }),
}));
