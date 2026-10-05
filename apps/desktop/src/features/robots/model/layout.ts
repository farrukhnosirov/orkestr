/**
 * Geometry of a robot window and its interactive shape (RR-C4). The shell
 * polls the cursor and enables mouse input only inside this shape, so
 * everything transparent around the robot stays click-through.
 *
 * All values are window-local CSS px.
 */

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Ellipse {
  cx: number;
  cy: number;
  rx: number;
  ry: number;
}

export interface HitShape {
  ellipses: Ellipse[];
  rects: Box[];
}

export interface RobotWindowLayout {
  width: number;
  height: number;
  /** The square the robot is drawn in. */
  robot: Box;
  /** Where the speech bubble may appear (above the robot). */
  bubble: Box;
}

/** Robot square at scale 1. */
export const ROBOT_BASE_SIZE = 150;
const BUBBLE_HEIGHT = 92;

/** Window size and regions for a design scale (0.6..1.6). */
export function robotWindowLayout(scale: number): RobotWindowLayout {
  const s = Math.round(ROBOT_BASE_SIZE * Math.min(1.6, Math.max(0.6, scale)));
  const width = Math.max(232, s + 48);
  const height = s + BUBBLE_HEIGHT + 16;
  return {
    width,
    height,
    robot: { x: Math.round((width - s) / 2), y: height - s - 4, w: s, h: s },
    bubble: { x: 8, y: 6, w: width - 16, h: BUBBLE_HEIGHT - 6 },
  };
}

/** Cluster (overflow) window. */
export const CLUSTER_SIZE = { width: 216, height: 76 } as const;

export interface HitShapeInput {
  layout: RobotWindowLayout;
  /** Measured bubble box (when a bubble is shown). */
  bubble: Box | null;
  /** The context menu is open: the whole window is interactive. */
  menuOpen: boolean;
}

/** The interactive shape: the robot body (ellipse), plus bubble / menu. */
export function hitShapeFor({ layout, bubble, menuOpen }: HitShapeInput): HitShape {
  if (menuOpen) return { ellipses: [], rects: [{ x: 0, y: 0, w: layout.width, h: layout.height }] };
  const r = layout.robot;
  const body: Ellipse = { cx: r.x + r.w / 2, cy: r.y + r.h * 0.52, rx: r.w * 0.34, ry: r.h * 0.46 };
  const rects = bubble ? [bubble] : [];
  return { ellipses: [body], rects };
}

/** Same test the shell runs (kept identical to `robots.rs::HitShape::contains`). */
export function hitTest(shape: HitShape, x: number, y: number): boolean {
  const inEllipse = (e: Ellipse) => {
    if (e.rx <= 0 || e.ry <= 0) return false;
    const dx = (x - e.cx) / e.rx;
    const dy = (y - e.cy) / e.ry;
    return dx * dx + dy * dy <= 1;
  };
  const inRect = (b: Box) => x >= b.x && y >= b.y && x <= b.x + b.w && y <= b.y + b.h;
  return shape.ellipses.some(inEllipse) || shape.rects.some(inRect);
}

/** Pointer travel (px) after which a press becomes a drag instead of a click. */
export const DRAG_THRESHOLD = 4;

export function isDragGesture(dx: number, dy: number): boolean {
  return Math.hypot(dx, dy) >= DRAG_THRESHOLD;
}

/** A drop zone in the main window (CSS px of its viewport). */
export interface DropZone {
  id: string;
  rect: Box;
}

/** Collects drop zones from elements marked `data-robot-drop="<projectId>"`. */
export function collectDropZones(root: ParentNode): DropZone[] {
  const zones: DropZone[] = [];
  root.querySelectorAll<HTMLElement>("[data-robot-drop]").forEach((el) => {
    const id = el.dataset["robotDrop"];
    if (!id) return;
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) return;
    zones.push({ id, rect: { x: r.left, y: r.top, w: r.width, h: r.height } });
  });
  return zones;
}
