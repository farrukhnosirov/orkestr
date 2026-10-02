/** Onboarding wizard view state (§55). Completion is persisted in settings. */
import { create } from "zustand";

export interface OnboardingState {
  open: boolean;
  /** Index into `ONBOARDING_STEPS`. */
  step: number;
  openWizard: (step?: number) => void;
  closeWizard: () => void;
  setStep: (step: number) => void;
}

export const useOnboardingStore = create<OnboardingState>()((set) => ({
  open: false,
  step: 0,
  openWizard: (step = 0) => set({ open: true, step }),
  closeWizard: () => set({ open: false }),
  setStep: (step) => set({ step }),
}));
