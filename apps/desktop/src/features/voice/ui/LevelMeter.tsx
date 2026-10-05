/** Segmented input level meter (Settings: mic test, wake-word enrollment). */
import { clsx } from "clsx";

export interface LevelMeterProps {
  /** 0..1 */
  level: number;
  /** Highlight as "speech detected". */
  active?: boolean;
  label: string;
  segments?: number;
}

export function LevelMeter({ level, active = false, label, segments = 24 }: LevelMeterProps) {
  const lit = Math.round(Math.min(1, Math.max(0, level)) * segments);
  return (
    <div
      className={clsx("vx-meter", active && "vx-meter--active")}
      role="meter"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(level * 100)}
    >
      {Array.from({ length: segments }, (_, i) => (
        <span
          key={i}
          className={clsx("vx-meter__seg", i < lit && "vx-meter__seg--on", i >= segments * 0.8 && "vx-meter__seg--hot")}
        />
      ))}
    </div>
  );
}
