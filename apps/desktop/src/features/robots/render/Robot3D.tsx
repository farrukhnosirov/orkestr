/**
 * Low-poly three.js robot (§30 "3D", Ultra Visual only). Lazy-loaded so
 * three.js never lands in the main or robot entry bundles; it draws the same
 * {@link RobotPose} as the SVG robot. Built from flat-shaded primitives — no
 * model files.
 */
import type { RobotDesign } from "@orkestr/protocol";
import { useEffect, useRef } from "react";
import {
  AmbientLight,
  CapsuleGeometry,
  Color,
  CylinderGeometry,
  DirectionalLight,
  Group,
  HemisphereLight,
  type Material,
  Mesh,
  MeshStandardMaterial,
  OctahedronGeometry,
  PerspectiveCamera,
  PointLight,
  Scene,
  SphereGeometry,
  BoxGeometry,
  WebGLRenderer,
} from "three";

import { accentOf, paletteOf } from "../model/design";
import type { RobotPose } from "../model/pose";
import { stateHex, type RobotState } from "../model/states";

export interface Robot3DProps {
  design: RobotDesign;
  state: RobotState;
  pose: RobotPose;
  size: number;
}

interface Rig {
  renderer: WebGLRenderer;
  scene: Scene;
  camera: PerspectiveCamera;
  root: Group;
  head: Group;
  armL: Group;
  armR: Group;
  eyes: Mesh[];
  eyeMat: MeshStandardMaterial;
  antennaMat: MeshStandardMaterial;
  rim: PointLight;
  star: Mesh;
  dispose: () => void;
}

const DEG = Math.PI / 180;

function buildRig(canvas: HTMLCanvasElement, design: RobotDesign): Rig {
  const pal = paletteOf(design);
  const renderer = new WebGLRenderer({ canvas, alpha: true, antialias: true, powerPreference: "low-power" });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
  renderer.setClearColor(0x000000, 0);
  const scene = new Scene();
  const camera = new PerspectiveCamera(30, 1, 0.1, 50);
  camera.position.set(0, 0.35, 7.2);
  camera.lookAt(0, 0.1, 0);

  const metal = design.material === "Metal";
  const matte = design.material === "Matte";
  const neon = design.material === "Neon";
  const bodyMat = new MeshStandardMaterial({
    color: new Color(neon ? pal.bodyDark : pal.bodyLight),
    flatShading: true,
    metalness: metal ? 0.85 : 0.15,
    roughness: matte ? 0.95 : metal ? 0.3 : 0.35,
    emissive: new Color(neon ? accentOf(design) : "#000000"),
    emissiveIntensity: neon ? 0.25 : 0,
  });
  const trimMat = new MeshStandardMaterial({ color: new Color(pal.trim), flatShading: true, metalness: 0.4, roughness: 0.45 });
  const screenMat = new MeshStandardMaterial({ color: new Color(pal.screen), roughness: 0.2, metalness: 0.1 });
  const eyeMat = new MeshStandardMaterial({ color: new Color("#ffffff"), emissive: new Color(accentOf(design)), emissiveIntensity: 1.4 });
  const antennaMat = eyeMat.clone();

  const root = new Group();
  scene.add(root);
  const torso = new Mesh(new CapsuleGeometry(0.62, 0.35, 3, 8), bodyMat);
  torso.position.y = -0.9;
  root.add(torso);
  const chest = new Mesh(new BoxGeometry(0.55, 0.32, 0.08), screenMat);
  chest.position.set(0, -0.85, 0.6);
  root.add(chest);

  const mkArm = (side: 1 | -1) => {
    const pivot = new Group();
    pivot.position.set(0.72 * side, -0.62, 0);
    const arm = new Mesh(new CapsuleGeometry(0.13, 0.45, 2, 6), bodyMat);
    arm.position.y = -0.32;
    const hand = new Mesh(new SphereGeometry(0.17, 8, 6), trimMat);
    hand.position.y = -0.66;
    pivot.add(arm, hand);
    root.add(pivot);
    return pivot;
  };
  const armL = mkArm(-1);
  const armR = mkArm(1);

  const head = new Group();
  head.position.y = 0.35;
  root.add(head);
  const skull = new Mesh(new SphereGeometry(0.85, 10, 7), bodyMat);
  skull.scale.set(1.28, 0.92, 0.95);
  head.add(skull);
  const face = new Mesh(new BoxGeometry(1.45, 0.82, 0.12), screenMat);
  face.position.set(0, -0.02, 0.76);
  head.add(face);
  const eyes = [-0.32, 0.32].map((x) => {
    const eye = new Mesh(new SphereGeometry(0.12, 10, 8), eyeMat);
    eye.position.set(x, 0.04, 0.85);
    head.add(eye);
    return eye;
  });
  for (const side of [-1, 1]) {
    const ear = new Mesh(new CylinderGeometry(0.17, 0.17, 0.16, 8), trimMat);
    ear.rotation.z = Math.PI / 2;
    ear.position.set(1.12 * side, 0, 0);
    head.add(ear);
  }
  if (design.accessories.includes("antenna")) {
    const stalk = new Mesh(new CylinderGeometry(0.035, 0.035, 0.4, 6), trimMat);
    stalk.position.y = 0.95;
    const ball = new Mesh(new SphereGeometry(0.11, 8, 6), antennaMat);
    ball.position.y = 1.2;
    head.add(stalk, ball);
  }
  const star = new Mesh(new OctahedronGeometry(0.2, 0), eyeMat);
  star.position.set(1.25, 1.35, 0);
  star.visible = false;
  root.add(star);

  scene.add(new HemisphereLight(0xdfe7ff, 0x1a1530, 1.1));
  scene.add(new AmbientLight(0xffffff, 0.25));
  const key = new DirectionalLight(0xffffff, 1.6);
  key.position.set(2.5, 3, 4);
  scene.add(key);
  const rim = new PointLight(new Color(accentOf(design)), 6, 10);
  rim.position.set(-2.2, 1.2, -1.5);
  scene.add(rim);

  const dispose = () => {
    scene.traverse((o) => {
      if (o instanceof Mesh) {
        o.geometry.dispose();
        const m = o.material as Material | Material[];
        (Array.isArray(m) ? m : [m]).forEach((x) => x.dispose());
      }
    });
    renderer.dispose();
  };
  return { renderer, scene, camera, root, head, armL, armR, eyes, eyeMat, antennaMat, rim, star, dispose };
}

export default function Robot3D({ design, state, pose, size }: Robot3DProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const rigRef = useRef<Rig | null>(null);

  // (Re)build when the look changes; pose updates are cheap per frame.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return undefined;
    const rig = buildRig(canvas, design);
    rigRef.current = rig;
    return () => {
      rig.dispose();
      rigRef.current = null;
    };
  }, [design]);

  useEffect(() => {
    rigRef.current?.renderer.setSize(size, size, false);
  }, [size]);

  useEffect(() => {
    const rig = rigRef.current;
    if (!rig) return;
    const color = new Color(stateHex(state, accentOf(design)));
    rig.root.position.set(pose.shakeX / 40, -pose.bodyY / 40 - 0.1, 0);
    rig.root.rotation.z = -pose.tilt * DEG;
    rig.root.scale.y = pose.squash;
    rig.head.rotation.z = -pose.headTilt * DEG;
    rig.head.rotation.y = pose.lookX * 0.25;
    rig.head.rotation.x = pose.lookY * 0.12;
    rig.armL.rotation.z = -pose.armL * DEG;
    rig.armR.rotation.z = pose.armR * DEG;
    // Negative angles bring the hands in front (typing).
    rig.armL.rotation.x = pose.armL < 0 ? pose.armL * DEG * 1.6 : 0;
    rig.armR.rotation.x = pose.armR < 0 ? pose.armR * DEG * 1.6 : 0;
    const open = pose.eyes === "closed" ? 0.12 : pose.eyes === "happy" || pose.eyes === "focus" ? 0.45 : Math.max(0.1, pose.eyeOpen);
    rig.eyes.forEach((e) => {
      e.scale.set(1, open, 1);
      e.position.x = Math.sign(e.position.x) * 0.32 + pose.lookX * 0.05;
    });
    rig.eyeMat.emissive = color;
    rig.antennaMat.emissive = color;
    rig.antennaMat.emissiveIntensity = 0.4 + pose.antenna * 1.6;
    rig.rim.color = color;
    rig.star.visible = state === "Completed";
    rig.star.rotation.y = pose.phase * Math.PI * 2;
    rig.renderer.render(rig.scene, rig.camera);
  }, [pose, state, design]);

  return <canvas ref={canvasRef} width={size} height={size} style={{ width: size, height: size, display: "block" }} />;
}
