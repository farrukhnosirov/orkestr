/**
 * Procedural 2D robot (SVG). Draws a {@link RobotPose} with a design's skin,
 * material, accent and accessories; every state has its own prop (holo
 * keyboard, test tube, magnifier, hourglass, sparks, confetti…). No bitmap
 * assets — everything is vector, so it is crisp at any scale and costs a few
 * dozen nodes.
 */
import type { RobotDesign } from "@orkestr/protocol";
import { memo, useId } from "react";

import { accentOf, paletteOf } from "../model/design";
import { hash01, type RobotPose } from "../model/pose";
import { stateColor, type RobotState } from "../model/states";

export interface RobotFigureProps {
  design: RobotDesign;
  state: RobotState;
  pose: RobotPose;
  asleep?: boolean;
  /** Glow filters (off in Performance / Battery Saver). */
  glow: boolean;
  /** Particles (confetti, sparks, bubbles). */
  particles: boolean;
  /** Pixel size of the square. */
  size: number;
  hovered?: boolean;
  className?: string;
}

const SHOULDER_L = { x: 66, y: 122 };
const SHOULDER_R = { x: 134, y: 122 };
const ARM_LEN = 38;

/** Hand position in body space for an arm angle (deg, positive = outward/up). */
export function handPosition(side: "L" | "R", angle: number): { x: number; y: number } {
  const a = ((side === "L" ? angle : -angle) * Math.PI) / 180;
  const s = side === "L" ? SHOULDER_L : SHOULDER_R;
  // SVG rotate(a) of the hanging vector (0, ARM_LEN).
  return { x: s.x - ARM_LEN * Math.sin(a), y: s.y + ARM_LEN * Math.cos(a) };
}

function Arm({ side, angle, fill, trim }: { side: "L" | "R"; angle: number; fill: string; trim: string }) {
  const s = side === "L" ? SHOULDER_L : SHOULDER_R;
  const rot = side === "L" ? angle : -angle;
  return (
    <g transform={`rotate(${rot.toFixed(2)} ${s.x} ${s.y})`}>
      <rect x={s.x - 7} y={s.y - 4} width={14} height={ARM_LEN} rx={7} fill={fill} stroke={trim} strokeOpacity={0.35} strokeWidth={1.2} />
      <circle cx={s.x} cy={s.y + ARM_LEN} r={8} fill={trim} />
      <circle cx={s.x} cy={s.y + ARM_LEN} r={3.2} fill="#ffffff" opacity={0.35} />
    </g>
  );
}

function Eye({ x, y, pose, color }: { x: number; y: number; pose: RobotPose; color: string }) {
  const cx = x + pose.lookX * 5;
  const cy = y + pose.lookY * 4;
  const stroke = { stroke: color, strokeWidth: 3.4, strokeLinecap: "round" as const, fill: "none" };
  switch (pose.eyes) {
    case "happy":
      return <path d={`M${cx - 7} ${cy + 3} Q${cx} ${cy - 7} ${cx + 7} ${cy + 3}`} {...stroke} />;
    case "x":
      return (
        <g {...stroke}>
          <path d={`M${cx - 5.5} ${cy - 5.5} L${cx + 5.5} ${cy + 5.5}`} />
          <path d={`M${cx + 5.5} ${cy - 5.5} L${cx - 5.5} ${cy + 5.5}`} />
        </g>
      );
    case "closed":
      return <path d={`M${cx - 7} ${cy} Q${cx} ${cy + 5} ${cx + 7} ${cy}`} {...stroke} />;
    case "focus": {
      const h = Math.max(1.5, 6 * pose.eyeOpen);
      return <rect x={cx - 8} y={cy - h / 2} width={16} height={h} rx={h / 2} fill={color} />;
    }
    default: {
      const ry = Math.max(0.8, 8.5 * pose.eyeOpen);
      return (
        <g>
          <ellipse cx={cx} cy={cy} rx={7.5} ry={ry} fill={color} />
          {pose.eyeOpen > 0.5 && <circle cx={cx - 2.6} cy={cy - 3.2} r={2.1} fill="#ffffff" opacity={0.85} />}
        </g>
      );
    }
  }
}

function Mouth({ pose, color }: { pose: RobotPose; color: string }) {
  const common = { stroke: color, strokeWidth: 2.6, strokeLinecap: "round" as const, fill: "none" };
  switch (pose.mouth) {
    case "smile":
      return <path d="M92 87 Q100 93 108 87" {...common} />;
    case "flat":
      return <path d="M93 89 L107 89" {...common} />;
    case "o":
      return <ellipse cx={100} cy={89} rx={3.6} ry={4.2} {...common} />;
    case "grin":
      return <path d="M89 85 Q100 98 111 85 Z" fill={color} opacity={0.9} />;
    case "frown":
      return <path d="M92 92 Q100 85 108 92" {...common} />;
    case "wave":
      return <path d="M90 89 Q95 85 100 89 T110 89" {...common} />;
  }
}

function Props({ pose, color, accent, trim, particles }: { pose: RobotPose; color: string; accent: string; trim: string; particles: boolean }) {
  const p = pose.phase;
  switch (pose.prop) {
    case "keyboard": {
      const lit = Math.floor(p * 10);
      return (
        <g>
          <rect x={58} y={158} width={84} height={17} rx={5} fill={color} fillOpacity={0.12} stroke={color} strokeOpacity={0.65} strokeWidth={1.2} />
          {Array.from({ length: 10 }, (_, i) => (
            <rect key={i} x={62 + (i % 5) * 16} y={161 + Math.floor(i / 5) * 7} width={12} height={4.5} rx={1.5} fill={color} opacity={i === lit || i === (lit + 3) % 10 ? 0.95 : 0.28} />
          ))}
          {particles &&
            [0, 1, 2].map((i) => {
              const q = (p + i / 3) % 1;
              return (
                <rect key={i} x={150 + i * 4} y={120 - q * 70} width={18 - i * 4} height={3} rx={1.5} fill={color} opacity={(1 - q) * 0.8} />
              );
            })}
        </g>
      );
    }
    case "tube": {
      const hand = handPosition("R", pose.armR);
      return (
        <g transform={`translate(${hand.x - 5} ${hand.y - 30}) rotate(${(pose.armR - 95) * 0.4} 5 30)`}>
          <rect x={0} y={0} width={10} height={30} rx={5} fill="#ffffff" fillOpacity={0.18} stroke="#ffffff" strokeOpacity={0.7} strokeWidth={1.3} />
          <rect x={1.5} y={12} width={7} height={16.5} rx={3.5} fill={color} opacity={0.9} />
          {particles &&
            [0, 1, 2].map((i) => {
              const q = (p + i * 0.33) % 1;
              return <circle key={i} cx={5 + Math.sin((q + i) * 9) * 2} cy={26 - q * 30} r={1.4 + (1 - q)} fill={color} opacity={1 - q} />;
            })}
        </g>
      );
    }
    case "magnifier": {
      const cx = 72 + p * 56;
      const cy = 76;
      return (
        <g>
          <line x1={cx + 11} y1={cy + 11} x2={cx + 22} y2={cy + 24} stroke={trim} strokeWidth={5} strokeLinecap="round" />
          <circle cx={cx} cy={cy} r={15} fill="#ffffff" fillOpacity={0.14} stroke={trim} strokeWidth={3.5} />
          <path d={`M${cx - 8} ${cy - 5} Q${cx - 6} ${cy - 9} ${cx - 2} ${cy - 10}`} stroke="#ffffff" strokeOpacity={0.8} strokeWidth={2} fill="none" strokeLinecap="round" />
        </g>
      );
    }
    case "hourglass": {
      const flip = p > 0.85 ? ((p - 0.85) / 0.15) * 180 : 0;
      const sand = p > 0.85 ? 0 : p / 0.85;
      return (
        <g transform={`translate(160 46) rotate(${flip.toFixed(1)})`}>
          <path d="M-9 -14 H9 L1.5 0 L9 14 H-9 L-1.5 0 Z" fill="#ffffff" fillOpacity={0.14} stroke={color} strokeWidth={1.6} strokeLinejoin="round" />
          <path d={`M${-7 + sand * 5.5} ${-12 + sand * 11} H${7 - sand * 5.5} L0 0 Z`} fill={color} opacity={0.9} />
          <path d={`M${-1.5 - sand * 6} 12 H${1.5 + sand * 6} L0 ${12 - sand * 10} Z`} fill={color} opacity={0.9} />
          <rect x={-11} y={-16} width={22} height={3} rx={1.5} fill={trim} />
          <rect x={-11} y={13} width={22} height={3} rx={1.5} fill={trim} />
        </g>
      );
    }
    case "sparks": {
      if (!particles) return null;
      const spots = [
        { x: 46, y: 40, r: -30 },
        { x: 156, y: 44, r: 30 },
        { x: 40, y: 96, r: -70 },
        { x: 162, y: 98, r: 70 },
      ];
      return (
        <g stroke={color} strokeWidth={2.4} strokeLinecap="round" strokeLinejoin="round" fill="none">
          {spots.map((s, i) => {
            const on = hash01(Math.floor(p * 8) * 7 + i) > 0.45;
            return on ? <path key={i} transform={`translate(${s.x} ${s.y}) rotate(${s.r})`} d="M0 -8 L3 -2 L-2 1 L1 8" /> : null;
          })}
        </g>
      );
    }
    case "confetti": {
      if (!particles) {
        return (
          <g transform="translate(156 36)">
            <circle r={11} fill={color} />
            <path d="M-5 0 L-1.5 4 L5.5 -4" stroke="#ffffff" strokeWidth={2.6} fill="none" strokeLinecap="round" strokeLinejoin="round" />
          </g>
        );
      }
      const colors = [color, accent, "#fbbf24", "#f472b6", "#a78bfa", "#60a5fa"];
      return (
        <g>
          {Array.from({ length: 16 }, (_, i) => {
            const q = (p + hash01(i + 50)) % 1;
            const x = 14 + hash01(i) * 172 + Math.sin((q + i) * 6) * 6;
            const y = -6 + q * 150;
            return (
              <rect
                key={i}
                x={x}
                y={y}
                width={5}
                height={9}
                rx={1.5}
                fill={colors[i % colors.length]}
                opacity={q < 0.85 ? 0.95 : (1 - q) * 6}
                transform={`rotate(${(q * 540 * (hash01(i + 9) - 0.5)).toFixed(1)} ${x + 2.5} ${y + 4.5})`}
              />
            );
          })}
          <g transform="translate(156 36)">
            <circle r={11} fill={color} />
            <path d="M-5 0 L-1.5 4 L5.5 -4" stroke="#ffffff" strokeWidth={2.6} fill="none" strokeLinecap="round" strokeLinejoin="round" />
          </g>
        </g>
      );
    }
    case "thought": {
      const dots = [
        { x: 140, y: 34, r: 3.2 },
        { x: 151, y: 22, r: 4.6 },
        { x: 166, y: 9, r: 6.4 },
      ];
      return (
        <g>
          {dots.map((d, i) => {
            const on = (p * 3 - i + 3) % 3;
            return <circle key={i} cx={d.x} cy={d.y} r={d.r} fill={color} opacity={0.25 + 0.7 * Math.max(0, 1 - Math.abs(on - 0.5))} />;
          })}
        </g>
      );
    }
    case "waves": {
      return (
        <g fill="none" stroke={color} strokeWidth={2.2} strokeLinecap="round">
          {[0, 1, 2].map((i) => {
            const q = (p + i / 3) % 1;
            const r = 8 + q * 16;
            return (
              <g key={i} opacity={(1 - q) * 0.85}>
                <path d={`M${44 - r * 0.5} ${76 - r} Q${40 - r} 76 ${44 - r * 0.5} ${76 + r}`} />
                <path d={`M${156 + r * 0.5} ${76 - r} Q${160 + r} 76 ${156 + r * 0.5} ${76 + r}`} />
              </g>
            );
          })}
        </g>
      );
    }
    case "sleep": {
      return (
        <g fill="none" stroke={color} strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
          {[0, 1].map((i) => {
            const q = (p + i / 2) % 1;
            const s = 0.7 + q * 0.6;
            return <path key={i} transform={`translate(${142 + q * 14} ${40 - q * 30}) scale(${s})`} d="M0 0 H8 L0 9 H8" opacity={1 - q} />;
          })}
        </g>
      );
    }
    default:
      return null;
  }
}

function RobotFigureImpl({ design, state, pose, asleep = false, glow, particles, size, hovered = false, className }: RobotFigureProps) {
  const uid = useId().replace(/[^a-zA-Z0-9]/g, "");
  const pal = paletteOf(design);
  const accent = accentOf(design);
  const color = stateColor(state, accent);
  const acc = new Set(design.accessories);
  const neon = design.material === "Neon";
  const body = `url(#b${uid})`;
  const edge = neon ? color : pal.bodyDark;
  const glowFilter = glow ? `url(#g${uid})` : undefined;
  const handsInFront = pose.armL < 0 || pose.armR < 0;
  const bodyTransform = `translate(${pose.shakeX.toFixed(2)} ${pose.bodyY.toFixed(2)}) translate(100 186) scale(1 ${pose.squash.toFixed(4)}) translate(-100 -186) rotate(${pose.tilt.toFixed(2)} 100 150)`;
  const arms = (
    <>
      <Arm side="L" angle={pose.armL} fill={body} trim={pal.trim} />
      <Arm side="R" angle={pose.armR} fill={body} trim={pal.trim} />
    </>
  );

  return (
    <svg
      className={className}
      width={size}
      height={size}
      viewBox="0 0 200 200"
      role="img"
      aria-hidden="true"
      style={{ overflow: "visible", filter: asleep ? "saturate(0.45) brightness(0.85)" : undefined }}
    >
      <defs>
        <linearGradient id={`b${uid}`} x1="0" y1="0" x2="0.25" y2="1">
          {design.material === "Metal" ? (
            <>
              <stop offset="0%" stopColor={pal.bodyLight} />
              <stop offset="30%" stopColor={pal.bodyDark} />
              <stop offset="48%" stopColor={pal.bodyLight} />
              <stop offset="70%" stopColor={pal.bodyDark} />
              <stop offset="100%" stopColor={pal.bodyLight} stopOpacity={0.85} />
            </>
          ) : (
            <>
              <stop offset="0%" stopColor={neon ? pal.bodyDark : pal.bodyLight} />
              <stop offset="100%" stopColor={neon ? pal.screen : pal.bodyDark} />
            </>
          )}
        </linearGradient>
        <radialGradient id={`s${uid}`} cx="0.5" cy="0.45" r="0.65">
          <stop offset="0%" stopColor={color} stopOpacity={0.32 * pose.glow} />
          <stop offset="100%" stopColor={color} stopOpacity={0} />
        </radialGradient>
        <radialGradient id={`h${uid}`} cx="0.5" cy="0.5" r="0.5">
          <stop offset="0%" stopColor={color} stopOpacity={hovered ? 0.4 : 0.22} />
          <stop offset="100%" stopColor={color} stopOpacity={0} />
        </radialGradient>
        {glow && (
          <filter id={`g${uid}`} x="-50%" y="-50%" width="200%" height="200%">
            <feGaussianBlur stdDeviation="2.6" result="blur" />
            <feMerge>
              <feMergeNode in="blur" />
              <feMergeNode in="SourceGraphic" />
            </feMerge>
          </filter>
        )}
      </defs>

      {/* aura + ground shadow */}
      {glow && <ellipse cx={100} cy={118} rx={92} ry={86} fill={`url(#h${uid})`} />}
      <ellipse cx={100} cy={191} rx={Math.max(26, 40 + pose.bodyY * 0.9)} ry={5.5} fill="#000000" opacity={0.32} />

      <g transform={bodyTransform}>
        {acc.has("jetpack") && (
          <g>
            <rect x={64} y={110} width={72} height={46} rx={14} fill={pal.trim} opacity={0.9} />
            {(state === "Coding" || state === "Completed") && particles && (
              <g filter={glowFilter}>
                <path d={`M74 156 Q78 ${170 + pose.phase * 6} 82 156 Z`} fill="#fbbf24" />
                <path d={`M118 156 Q122 ${170 + (1 - pose.phase) * 6} 126 156 Z`} fill="#fbbf24" />
              </g>
            )}
          </g>
        )}
        {!handsInFront && arms}

        {/* torso */}
        <rect x={64} y={106} width={72} height={66} rx={25} fill={body} stroke={edge} strokeOpacity={neon ? 0.95 : 0.5} strokeWidth={neon ? 2 : 1.4} filter={neon ? glowFilter : undefined} />
        <rect x={81} y={122} width={38} height={26} rx={9} fill={pal.screen} stroke={pal.trim} strokeOpacity={0.6} strokeWidth={1.2} />
        {state === "Completed" ? (
          <path d="M100 143 C92 137 88 133 91 129 C93 126 97 127 100 130 C103 127 107 126 109 129 C112 133 108 137 100 143 Z" fill={color} filter={glowFilter} />
        ) : (
          [0, 1, 2, 3].map((i) => {
            const h = asleep ? 3 : 4 + Math.abs(Math.sin(pose.phase * Math.PI * 2 + i * 1.3)) * 12 * (state === "Idle" ? 0.4 : 1);
            return <rect key={i} x={86 + i * 7.5} y={144 - h} width={4.5} height={h} rx={2} fill={color} opacity={0.85} />;
          })
        )}
        <circle cx={100} cy={161} r={3} fill={pal.trim} opacity={0.8} />
        {acc.has("scarf") && (
          <g>
            <rect x={78} y={102} width={44} height={11} rx={5.5} fill={accent} />
            <path d="M112 108 L120 128 L112 126 Z" fill={accent} opacity={0.85} />
          </g>
        )}
        {acc.has("bowtie") && (
          <g transform="translate(100 111)">
            <path d="M0 0 L-11 -6 L-11 6 Z" fill={accent} />
            <path d="M0 0 L11 -6 L11 6 Z" fill={accent} />
            <circle r={3} fill={pal.trim} />
          </g>
        )}
        {handsInFront && arms}
        {pose.prop === "tube" && <Props pose={pose} color={color} accent={accent} trim={pal.trim} particles={particles} />}

        {/* head */}
        <g transform={`translate(0 ${pose.headY.toFixed(2)}) rotate(${pose.headTilt.toFixed(2)} 100 104)`}>
          <rect x={90} y={100} width={20} height={9} rx={3} fill={pal.trim} />
          <circle cx={51} cy={74} r={9} fill={pal.trim} />
          <circle cx={149} cy={74} r={9} fill={pal.trim} />
          <circle cx={51} cy={74} r={3.6} fill={color} opacity={0.8} filter={glowFilter} />
          <circle cx={149} cy={74} r={3.6} fill={color} opacity={0.8} filter={glowFilter} />
          {acc.has("antenna") && (
            <g>
              <line x1={100} y1={38} x2={100} y2={20} stroke={pal.trim} strokeWidth={3.2} strokeLinecap="round" />
              {glow && <circle cx={100} cy={16} r={11} fill={color} opacity={0.28 * pose.antenna} />}
              <circle cx={100} cy={16} r={5.6} fill={color} opacity={0.35 + 0.65 * pose.antenna} filter={glowFilter} />
            </g>
          )}
          <rect x={52} y={36} width={96} height={70} rx={30} fill={body} stroke={edge} strokeOpacity={neon ? 0.95 : 0.5} strokeWidth={neon ? 2 : 1.4} filter={neon ? glowFilter : undefined} />
          {design.material === "Glossy" && <ellipse cx={78} cy={47} rx={19} ry={6.5} fill="#ffffff" opacity={0.5} transform="rotate(-12 78 47)" />}
          {design.material === "Metal" && <rect x={58} y={41} width={84} height={3} rx={1.5} fill="#ffffff" opacity={0.45} />}
          <rect x={61} y={49} width={78} height={48} rx={20} fill={pal.screen} />
          <rect x={61} y={49} width={78} height={48} rx={20} fill={`url(#s${uid})`} />
          {state === "Error" && <rect x={61} y={49} width={78} height={48} rx={20} fill={color} opacity={0.12 + Math.max(0, pose.glow - 0.5) * 0.3} />}
          <g filter={glowFilter}>
            <Eye x={84} y={72} pose={pose} color={color} />
            <Eye x={116} y={72} pose={pose} color={color} />
            <Mouth pose={pose} color={color} />
          </g>
          {acc.has("visor") && <rect x={57} y={60} width={86} height={24} rx={12} fill={color} fillOpacity={0.16} stroke={color} strokeOpacity={0.55} strokeWidth={1.4} />}
          {acc.has("headphones") && (
            <g>
              <path d="M48 70 Q48 24 100 24 Q152 24 152 70" stroke={pal.trim} strokeWidth={6} fill="none" strokeLinecap="round" />
              <rect x={40} y={60} width={15} height={28} rx={7} fill={pal.trim} />
              <rect x={145} y={60} width={15} height={28} rx={7} fill={pal.trim} />
              <rect x={43} y={66} width={4} height={16} rx={2} fill={color} opacity={0.75} />
              <rect x={153} y={66} width={4} height={16} rx={2} fill={color} opacity={0.75} />
            </g>
          )}
          {acc.has("halo") && <ellipse cx={100} cy={acc.has("antenna") ? 6 : 26} rx={26} ry={6} fill="none" stroke="#fde68a" strokeWidth={3} opacity={0.9} filter={glowFilter} />}
        </g>
      </g>

      {pose.prop !== "tube" && <Props pose={pose} color={color} accent={accent} trim={pal.trim} particles={particles} />}
    </svg>
  );
}

export const RobotFigure = memo(RobotFigureImpl);
