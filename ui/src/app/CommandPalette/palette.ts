import { create } from 'zustand';

interface PaletteState {
  visible: boolean;
  open: () => void;
  close: () => void;
  toggle: () => void;
}

/** Whether the command palette is showing. */
export const usePalette = create<PaletteState>((set) => ({
  visible: false,
  open: () => set({ visible: true }),
  close: () => set({ visible: false }),
  toggle: () => set((state) => ({ visible: !state.visible })),
}));
