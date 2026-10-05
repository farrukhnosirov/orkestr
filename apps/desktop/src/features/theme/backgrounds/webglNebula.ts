/**
 * WebGL nebula (Ultra Visual only, §31): one full-screen fragment shader
 * (fbm noise, 3 palette colors). Raw WebGL instead of three.js keeps this
 * chunk tiny; it is code-split and loaded only when the renderer is `webgl`.
 * Returns `null` when WebGL is unavailable — the caller falls back to 2D.
 */
import type { SceneHandle, SceneParams } from "./scene";
import { frameDue } from "./scene";

const VERT = "attribute vec2 p; void main(){ gl_Position = vec4(p, 0.0, 1.0); }";

const FRAG = [
  "precision mediump float;",
  "uniform vec2 res; uniform float t; uniform float k;",
  "uniform vec3 c0; uniform vec3 c1; uniform vec3 c2;",
  "float h(vec2 p){ return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }",
  "float n(vec2 p){ vec2 i = floor(p), f = fract(p); f = f*f*(3.0-2.0*f);",
  "  return mix(mix(h(i), h(i+vec2(1,0)), f.x), mix(h(i+vec2(0,1)), h(i+vec2(1,1)), f.x), f.y); }",
  "float fbm(vec2 p){ float v = 0.0, a = 0.5; for (int i = 0; i < 5; i++){ v += a*n(p); p *= 2.03; a *= 0.5; } return v; }",
  "void main(){",
  "  vec2 uv = gl_FragCoord.xy / res; vec2 p = uv * vec2(res.x/res.y, 1.0) * 2.2;",
  "  float q = fbm(p + vec2(t*0.03, -t*0.02));",
  "  float r = fbm(p + q*1.7 + vec2(-t*0.02, t*0.025));",
  "  float s = fbm(p*0.6 + r + t*0.01);",
  "  vec3 col = c0 * smoothstep(0.35, 0.9, r) + c1 * smoothstep(0.4, 1.0, s) * 0.9 + c2 * smoothstep(0.55, 1.0, q) * 0.7;",
  "  float vign = smoothstep(1.25, 0.2, length(uv - 0.5));",
  "  vec3 c = col * vign; float m = max(max(c.r, c.g), c.b);",
  "  gl_FragColor = vec4(c / max(m, 0.001), clamp(m * k * 0.7, 0.0, 0.85));",
  "}",
].join("\n");

function compile(gl: WebGLRenderingContext, type: number, src: string): WebGLShader | null {
  const s = gl.createShader(type);
  if (!s) return null;
  gl.shaderSource(s, src);
  gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
    console.warn("nebula shader", gl.getShaderInfoLog(s));
    gl.deleteShader(s);
    return null;
  }
  return s;
}

export function runWebglNebula(canvas: HTMLCanvasElement, initial: SceneParams): SceneHandle | null {
  const gl = canvas.getContext("webgl", { alpha: true, premultipliedAlpha: false, antialias: false, powerPreference: "low-power" });
  if (!gl) return null;
  const vs = compile(gl, gl.VERTEX_SHADER, VERT);
  const fs = compile(gl, gl.FRAGMENT_SHADER, FRAG);
  const prog = gl.createProgram();
  if (!vs || !fs || !prog) return null;
  gl.attachShader(prog, vs);
  gl.attachShader(prog, fs);
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) return null;
  gl.useProgram(prog);
  const buf = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(prog, "p");
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
  const u = (name: string) => gl.getUniformLocation(prog, name);
  const uRes = u("res");
  const uT = u("t");
  const uK = u("k");
  const uC = [u("c0"), u("c1"), u("c2")];

  let params = initial;
  let raf = 0;
  let last: number | null = null;
  let disposed = false;
  const start = performance.now();

  const size = () => {
    // The shader is the most expensive renderer: never above 1.5 DPR, and 3/4 of that.
    const dpr = Math.min(1.5, window.devicePixelRatio || 1) * params.resolution * 0.75;
    canvas.width = Math.max(1, Math.round(canvas.clientWidth * dpr));
    canvas.height = Math.max(1, Math.round(canvas.clientHeight * dpr));
    gl.viewport(0, 0, canvas.width, canvas.height);
  };
  const draw = (now: number) => {
    gl.uniform2f(uRes, canvas.width, canvas.height);
    gl.uniform1f(uT, ((now - start) / 1000) * params.speed * 4);
    gl.uniform1f(uK, params.intensity);
    uC.forEach((l, i) => {
      const c = params.colors[i % params.colors.length] ?? { r: 34, g: 211, b: 238, a: 1 };
      gl.uniform3f(l, c.r / 255, c.g / 255, c.b / 255);
    });
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  };
  const loop = (now: number) => {
    raf = 0;
    if (disposed || document.hidden) return;
    if (frameDue(now, last, params.fps)) {
      last = now;
      draw(now);
    }
    if (params.fps > 0) raf = requestAnimationFrame(loop);
  };
  const kick = () => {
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
    last = null;
    if (params.fps > 0) raf = requestAnimationFrame(loop);
    else draw(performance.now());
  };
  const ro = new ResizeObserver(() => {
    size();
    if (params.fps <= 0) draw(performance.now());
  });
  ro.observe(canvas);
  const onVisibility = () => {
    if (!document.hidden) kick();
  };
  document.addEventListener("visibilitychange", onVisibility);
  size();
  kick();

  return {
    update: (p) => {
      const resized = p.resolution !== params.resolution;
      params = p;
      if (resized) size();
      kick();
    },
    dispose: () => {
      disposed = true;
      if (raf) cancelAnimationFrame(raf);
      ro.disconnect();
      document.removeEventListener("visibilitychange", onVisibility);
      gl.getExtension("WEBGL_lose_context")?.loseContext();
    },
  };
}
