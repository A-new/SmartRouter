// Navigation store — app-wide navigation state & signals
// Trimmed to the Free Models + Smart Router feature set.

import { create } from 'zustand';

/** The app ships a single page; a union so more can be added back later. */
export type PageType = 'freeModels';

interface NavigationState {
  activePage: PageType;

  setActivePage: (page: PageType) => void;
}

export const useNavigationStore = create<NavigationState>((set) => ({
  activePage: 'freeModels',

  setActivePage: (page) => set({ activePage: page }),
}));
