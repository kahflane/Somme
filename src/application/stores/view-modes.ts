import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";

export type ViewMode = "grid" | "list";
export type SortOrder = "az" | "za" | "newest" | "oldest";

interface PagePrefs {
  view: ViewMode;
  sort: SortOrder;
}

const defaults: PagePrefs = { view: "grid", sort: "az" };

interface ViewModesState {
  pages: Record<string, PagePrefs>;
  update: (page: string, patch: Partial<PagePrefs>) => void;
}

export const useViewModes = create<ViewModesState>()(
  persist(
    (set) => ({
      pages: {},
      update: (page, patch) =>
        set((state) => ({
          pages: { ...state.pages, [page]: { ...defaults, ...state.pages[page], ...patch } },
        })),
    }),
    {
      name: "somme.view-modes",
      version: 2,
      storage: createJSONStorage(() => localStorage),
      partialize: ({ pages }) => ({ pages }),
      migrate: () => ({ pages: {} }),
    },
  ),
);

export function useViewMode(page: string): [ViewMode, (view: ViewMode) => void] {
  const view = useViewModes((state) => state.pages[page]?.view ?? defaults.view);
  const update = useViewModes((state) => state.update);
  return [view, (next) => update(page, { view: next })];
}

export function useSortOrder(page: string): [SortOrder, (sort: SortOrder) => void] {
  const sort = useViewModes((state) => state.pages[page]?.sort ?? defaults.sort);
  const update = useViewModes((state) => state.update);
  return [sort, (next) => update(page, { sort: next })];
}

interface Sortable {
  createdAt: number;
}

export function sortItems<T extends Sortable>(items: T[], order: SortOrder, name: (item: T) => string): T[] {
  const sorted = [...items];
  switch (order) {
    case "az":
      return sorted.sort((left, right) => name(left).localeCompare(name(right)));
    case "za":
      return sorted.sort((left, right) => name(right).localeCompare(name(left)));
    case "newest":
      return sorted.sort((left, right) => right.createdAt - left.createdAt);
    case "oldest":
      return sorted.sort((left, right) => left.createdAt - right.createdAt);
  }
}
