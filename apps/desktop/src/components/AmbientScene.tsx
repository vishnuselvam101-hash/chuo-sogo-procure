import { useEffect, useRef } from "react";

/*
 * Sign-in backdrop: a soft light field over a perspective "layout floor", drawn by one small
 * WebGL2 fragment shader (no library, nothing loaded from the network). Decoration only:
 * aria-hidden, never takes focus or input, rendered at reduced resolution and capped at
 * ~30 fps, paused when the window is hidden, a single still frame under
 * prefers-reduced-motion, and nothing at all (the CSS gradient stays) without WebGL2.
 */
const VERTEX = `#version 300 es
in vec2 a;
void main() { gl_Position = vec4(a, 0.0, 1.0); }`;

const FRAGMENT = `#version 300 es
precision mediump float;
uniform vec2 uRes;
uniform float uTime;
uniform float uDark;
out vec4 o;
float hash(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
float noise(vec2 p) {
  vec2 i = floor(p), f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), f.x),
             mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), f.x), f.y);
}
float fbm(vec2 p) {
  float v = 0.0, a = 0.5;
  for (int i = 0; i < 4; i++) { v += a * noise(p); p *= 2.02; a *= 0.5; }
  return v;
}
void main() {
  vec2 uv = gl_FragCoord.xy / uRes;
  vec2 p = (gl_FragCoord.xy - 0.5 * uRes) / uRes.y;
  float t = uTime * 0.035;
  vec3 low = mix(vec3(0.905, 0.925, 0.94), vec3(0.03, 0.05, 0.11), uDark);
  vec3 high = mix(vec3(0.975, 0.98, 0.985), vec3(0.075, 0.11, 0.21), uDark);
  vec3 col = mix(low, high, smoothstep(0.0, 1.0, uv.y));
  vec3 blue = vec3(0.133, 0.294, 0.824);
  vec3 sky = vec3(0.42, 0.64, 0.97);
  float f = fbm(p * 1.25 + vec2(t, -0.6 * t));
  float g = fbm(p * 2.0 - vec2(0.7 * t, 0.3 * t) + f);
  col += mix(0.11, 0.26, uDark) * blue * smoothstep(0.38, 0.9, g);
  col += mix(0.07, 0.12, uDark) * sky * smoothstep(0.5, 0.95, f) * (1.0 - uv.y);
  float horizon = -0.16;
  float d = horizon - p.y;
  if (d > 0.0) {
    vec2 q = vec2(p.x / d, 1.0 / d + uTime * 0.12) * vec2(0.85, 0.55);
    vec2 w = fwidth(q);
    vec2 gl = abs(fract(q) - 0.5) / max(w, vec2(1e-4));
    float line = 1.0 - min(min(gl.x, gl.y), 1.0);
    float fade = smoothstep(0.015, 0.32, d) * (1.0 - smoothstep(0.35, 0.75, d) * 0.6);
    col += mix(0.16, 0.30, uDark) * mix(blue, sky, 0.4) * line * fade;
  }
  col += mix(0.10, 0.18, uDark) * sky * exp(-abs(d) * 16.0);
  float vig = smoothstep(1.25, 0.35, length(p * vec2(0.8, 1.0)));
  col *= mix(0.94, 0.82, uDark) + (1.0 - mix(0.94, 0.82, uDark)) * vig;
  o = vec4(col, 1.0);
}`;

const SCALE = 0.5;
const FRAME_MS = 33;

function compile(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader | null {
  const s = gl.createShader(type);
  if (!s) return null;
  gl.shaderSource(s, src);
  gl.compileShader(s);
  return gl.getShaderParameter(s, gl.COMPILE_STATUS) ? s : null;
}

export default function AmbientScene() {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas || typeof window.matchMedia !== "function") return;
    let gl: WebGL2RenderingContext | null = null;
    try {
      gl = canvas.getContext("webgl2", { antialias: false, alpha: false, powerPreference: "low-power" });
    } catch {
      gl = null;
    }
    if (!gl) return;
    const vs = compile(gl, gl.VERTEX_SHADER, VERTEX);
    const fs = compile(gl, gl.FRAGMENT_SHADER, FRAGMENT);
    const prog = gl.createProgram();
    if (!vs || !fs || !prog) return;
    gl.attachShader(prog, vs);
    gl.attachShader(prog, fs);
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) return;
    gl.useProgram(prog);
    const buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    const loc = gl.getAttribLocation(prog, "a");
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
    const uRes = gl.getUniformLocation(prog, "uRes");
    const uTime = gl.getUniformLocation(prog, "uTime");
    const uDark = gl.getUniformLocation(prog, "uDark");

    const still = window.matchMedia("(prefers-reduced-motion: reduce)");
    const dark = window.matchMedia("(prefers-color-scheme: dark)");
    const start = performance.now();
    let raf = 0;
    let last = 0;

    const draw = (now: number) => {
      if (!gl) return;
      const w = Math.max(1, Math.round(canvas.clientWidth * SCALE));
      const h = Math.max(1, Math.round(canvas.clientHeight * SCALE));
      if (canvas.width !== w || canvas.height !== h) {
        canvas.width = w;
        canvas.height = h;
        gl.viewport(0, 0, w, h);
      }
      gl.uniform2f(uRes, w, h);
      gl.uniform1f(uTime, still.matches ? 12 : (now - start) / 1000);
      gl.uniform1f(uDark, dark.matches ? 1 : 0);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      canvas.dataset.ready = "true";
    };
    const loop = (now: number) => {
      raf = requestAnimationFrame(loop);
      if (document.hidden || now - last < FRAME_MS) return;
      last = now;
      draw(now);
    };
    const restart = () => {
      cancelAnimationFrame(raf);
      if (still.matches) draw(performance.now());
      else raf = requestAnimationFrame(loop);
    };
    restart();
    still.addEventListener("change", restart);
    dark.addEventListener("change", restart);
    window.addEventListener("resize", restart);
    return () => {
      cancelAnimationFrame(raf);
      still.removeEventListener("change", restart);
      dark.removeEventListener("change", restart);
      window.removeEventListener("resize", restart);
      gl?.getExtension("WEBGL_lose_context")?.loseContext();
    };
  }, []);

  return <canvas ref={ref} className="ambient-scene" aria-hidden="true" tabIndex={-1} />;
}
