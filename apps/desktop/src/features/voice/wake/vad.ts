/**
 * Energy voice-activity detector with an adaptive noise floor (ADR-0017 VAD
 * gate). Frame-by-frame on dBFS energies: speech starts after `onsetFrames`
 * loud frames, ends after `hangoverFrames` quiet ones. Pure, unit-tested.
 */

export interface VadOptions {
  /** dB above the noise floor that counts as speech. */
  marginDb: number;
  /** Absolute minimum level for speech, dBFS. */
  minDb: number;
  onsetFrames: number;
  hangoverFrames: number;
  /** Noise floor tracking speed (0..1) when quiet. */
  floorAdapt: number;
}

export const DEFAULT_VAD: VadOptions = { marginDb: 10, minDb: -55, onsetFrames: 3, hangoverFrames: 25, floorAdapt: 0.05 };

export type VadEvent = "start" | "end" | null;

export class EnergyVad {
  private floor = -70;
  private loud = 0;
  private quiet = 0;
  speaking = false;

  constructor(private readonly opts: VadOptions = DEFAULT_VAD) {}

  get noiseFloorDb(): number {
    return this.floor;
  }

  /** Feeds one frame energy; returns a transition, if any. */
  push(energyDb: number): VadEvent {
    const { marginDb, minDb, onsetFrames, hangoverFrames, floorAdapt } = this.opts;
    const isLoud = energyDb > Math.max(minDb, this.floor + marginDb);
    if (!isLoud) {
      // Track the floor while quiet: fast downwards, slow upwards.
      const rate = energyDb < this.floor ? 0.3 : floorAdapt;
      this.floor += (energyDb - this.floor) * rate;
    }
    if (this.speaking) {
      this.quiet = isLoud ? 0 : this.quiet + 1;
      if (this.quiet >= hangoverFrames) {
        this.speaking = false;
        this.loud = 0;
        this.quiet = 0;
        return "end";
      }
      return null;
    }
    this.loud = isLoud ? this.loud + 1 : 0;
    if (this.loud >= onsetFrames) {
      this.speaking = true;
      this.quiet = 0;
      return "start";
    }
    return null;
  }

  reset(): void {
    this.floor = -70;
    this.loud = 0;
    this.quiet = 0;
    this.speaking = false;
  }
}
