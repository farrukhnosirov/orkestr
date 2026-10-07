/**
 * Whether the Orkestr trash dialog is open. A plain flag in a store (not a
 * request counter) so the "Open Trash" command works even when the Files
 * panel mounts only after the command ran.
 */
import { create } from "zustand";

interface TrashUiState {
  open: boolean;
  setOpen: (open: boolean) => void;
}

export const useTrashUi = create<TrashUiState>((set) => ({
  open: false,
  setOpen: (open) => set({ open }),
}));
