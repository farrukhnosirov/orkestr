/** View state of Consensus Mode: whether the dialog is open (opened by the Manager panel or the command palette). */
import { create } from "zustand";

export interface ConsensusUiState {
  open: boolean;
  setOpen: (open: boolean) => void;
}

export const useConsensusUi = create<ConsensusUiState>()((set) => ({
  open: false,
  setOpen: (open) => set({ open }),
}));
