import { useId } from "react";

/** Orkestr mark: a core with a tilted orbit (matches the app icon). */
export function Logo({ size = 22 }: { size?: number }) {
  const id = useId().replace(/:/g, "");
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" aria-hidden="true" className="ok-logo">
      <defs>
        <linearGradient id={`${id}-orbit`} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor="var(--ok-color-accent)" />
          <stop offset="100%" stopColor="var(--ok-color-violet)" />
        </linearGradient>
        <radialGradient id={`${id}-core`} cx="50%" cy="45%" r="55%">
          <stop offset="0%" stopColor="#ffffff" />
          <stop offset="100%" stopColor="var(--ok-color-accent)" />
        </radialGradient>
      </defs>
      <ellipse
        cx="16"
        cy="16"
        rx="13"
        ry="5.4"
        fill="none"
        stroke={`url(#${id}-orbit)`}
        strokeWidth="2.2"
        transform="rotate(-28 16 16)"
      />
      <circle cx="16" cy="16" r="4.6" fill={`url(#${id}-core)`} />
      <circle cx="24.3" cy="20.6" r="1.9" fill="var(--ok-color-violet)" />
    </svg>
  );
}
