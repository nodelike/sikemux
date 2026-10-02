import { createContext, useContext, useEffect, useRef, useState } from 'react';
import { AccessibilityInfo, AppState, Image, PixelRatio, StyleSheet, View } from 'react-native';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';
import { GLView, type ExpoWebGLRenderingContext } from 'expo-gl';
import { useIsFocused } from 'expo-router';
import { ditheringFragmentShader, getShaderColorFromString, imageDitheringFragmentShader } from '@paper-design/shaders';

import type { DeviceBackdrop } from '@/devices/backdrop';
import { useColors, type Palette } from './theme';
import { uploadPicture } from './picture';
import { vertexShaderSource } from './vertexShader.generated';

/** What the Mac a screen belongs to draws behind its panes; absent outside a device's screens. */
export const BackdropContext = createContext<DeviceBackdrop | undefined>(undefined);

/** Every backdrop reads one clock, so the grain carries on across screens instead of restarting. */
const ORIGIN = Date.now();
/** The Mac ticks its grain at 30 fps, 20 on battery; a phone is always on battery. */
const FRAME_MS = 50;
/** Where the grain rests when motion is reduced, as on the Mac. */
const STILL_MS = 2500;

type Uniform = number | boolean | number[];

type Loop = { start: () => void; stop: () => void };

/** The Mac's `ambient` and `image` presets from src/lib/shaderField.ts. */
function preset(colors: Palette, image: boolean): { fragment: string; speed: number; uniforms: Record<string, Uniform> } {
  const sizing = { u_originX: 0.5, u_originY: 0.5, u_worldWidth: 0, u_worldHeight: 0, u_rotation: 0, u_offsetX: 0, u_offsetY: 0 };
  const clear = [0, 0, 0, 0];
  if (image) {
    const ink = getShaderColorFromString(colors.ink);
    return {
      fragment: imageDitheringFragmentShader,
      speed: 0,
      uniforms: {
        ...sizing,
        u_colorBack: clear,
        u_colorFront: ink,
        u_colorHighlight: ink,
        u_originalColors: true,
        u_inverted: false,
        u_type: 4,
        u_pxSize: 2,
        u_colorSteps: 4,
        u_fit: 2,
        u_scale: 1,
      },
    };
  }
  return {
    fragment: ditheringFragmentShader,
    speed: 0.5,
    uniforms: {
      ...sizing,
      u_colorBack: clear,
      u_colorFront: getShaderColorFromString(colors.shaderDot),
      u_shape: 1,
      u_type: 4,
      u_pxSize: 3,
      u_fit: 0,
      u_scale: 2.4,
    },
  };
}

/** A phone GPU's medium precision is too coarse for the noise; the library raises it the same way. */
function highPrecision(gl: ExpoWebGLRenderingContext, source: string): string {
  const format = gl.getShaderPrecisionFormat(gl.FRAGMENT_SHADER, gl.MEDIUM_FLOAT);
  if (!format || format.precision >= 23) return source;
  return source
    .replace(/precision\s+(lowp|mediump)\s+float/g, 'precision highp float')
    .replace(/\b(uniform|varying|attribute)\s+(lowp|mediump)\s+(\w+)/g, '$1 highp $3');
}

function compile(gl: ExpoWebGLRenderingContext, type: number, source: string) {
  const shader = gl.createShader(type);
  if (!shader) return null;
  gl.shaderSource(shader, highPrecision(gl, source));
  gl.compileShader(shader);
  if (gl.getShaderParameter(shader, gl.COMPILE_STATUS)) return shader;
  console.warn('Backdrop shader did not compile:', gl.getShaderInfoLog(shader));
  return null;
}

type Surface = {
  gl: ExpoWebGLRenderingContext;
  program: WebGLProgram;
  buffer: WebGLBuffer | null;
  position: number;
  time: WebGLUniformLocation | null;
};

function mount(gl: ExpoWebGLRenderingContext, fragment: string, uniforms: Record<string, Uniform>): Surface | null {
  const vertex = compile(gl, gl.VERTEX_SHADER, vertexShaderSource);
  const pixels = compile(gl, gl.FRAGMENT_SHADER, fragment);
  const program = gl.createProgram();
  if (!vertex || !pixels || !program) return null;
  gl.attachShader(program, vertex);
  gl.attachShader(program, pixels);
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    console.warn('Backdrop shader did not link:', gl.getProgramInfoLog(program));
    return null;
  }
  gl.useProgram(program);
  const position = gl.getAttribLocation(program, 'a_position');
  const buffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]), gl.STATIC_DRAW);
  gl.enableVertexAttribArray(position);
  gl.vertexAttribPointer(position, 2, gl.FLOAT, false, 0, 0);

  gl.viewport(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight);
  const set: Record<string, Uniform> = {
    ...uniforms,
    u_resolution: [gl.drawingBufferWidth, gl.drawingBufferHeight],
    u_pixelRatio: PixelRatio.get(),
  };
  for (const [name, value] of Object.entries(set)) {
    const location = gl.getUniformLocation(program, name);
    if (!location) continue;
    if (typeof value === 'boolean') gl.uniform1i(location, value ? 1 : 0);
    else if (typeof value === 'number') gl.uniform1f(location, value);
    else if (value.length === 2) gl.uniform2fv(location, value);
    else gl.uniform4fv(location, value);
  }
  return { gl, program, buffer, position, time: gl.getUniformLocation(program, 'u_time') };
}

/** expo-gl presents a frame with GL calls of its own, so each frame binds everything again. */
function draw(surface: Surface, frameMs: number) {
  const { gl } = surface;
  gl.useProgram(surface.program);
  gl.bindBuffer(gl.ARRAY_BUFFER, surface.buffer);
  gl.enableVertexAttribArray(surface.position);
  gl.vertexAttribPointer(surface.position, 2, gl.FLOAT, false, 0, 0);
  gl.viewport(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight);
  gl.clearColor(0, 0, 0, 0);
  gl.enable(gl.BLEND);
  gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
  if (surface.time) gl.uniform1f(surface.time, frameMs * 1e-3);
  gl.clear(gl.COLOR_BUFFER_BIT);
  gl.drawArrays(gl.TRIANGLES, 0, 6);
  gl.flush();
  gl.endFrameEXP();
}

/** Whether the person asked the system to reduce motion. */
export function useStill(): boolean {
  const [still, setStill] = useState(false);
  useEffect(() => {
    AccessibilityInfo.isReduceMotionEnabled().then(setStill);
    const listener = AccessibilityInfo.addEventListener('reduceMotionChanged', setStill);
    return () => listener.remove();
  }, []);
  return still;
}

function useAppActive(): boolean {
  const [active, setActive] = useState(AppState.currentState === 'active');
  useEffect(() => {
    const listener = AppState.addEventListener('change', (state) => setActive(state === 'active'));
    return () => listener.remove();
  }, []);
  return active;
}

/**
 * The Mac's pane backdrop at the top of a device's screen: its moving dithered
 * grain, or its picture dithered in place, faded out down the screen as on the Mac.
 */
export function Backdrop() {
  const backdrop = useContext(BackdropContext);
  const colors = useColors();
  const focused = useIsFocused();
  const active = useAppActive();
  const still = useStill();
  const [aspect, setAspect] = useState<number>();
  const picture = backdrop?.texture ? backdrop.image : undefined;
  const moving = !picture && focused && active && !still;
  const loops = useRef(new Set<Loop>());
  const movingNow = useRef(moving);

  useEffect(() => {
    if (!picture) return;
    Image.getSize(
      picture,
      (width, height) => setAspect(width / height),
      () => setAspect(undefined),
    );
  }, [picture]);

  useEffect(() => {
    movingNow.current = moving;
    loops.current.forEach((loop) => (moving ? loop.start() : loop.stop()));
  }, [moving]);

  useEffect(() => {
    const running = loops.current;
    return () => running.forEach((loop) => loop.stop());
  }, []);

  if (!backdrop?.texture || (picture && !aspect)) return null;
  const { fragment, speed, uniforms } = preset(colors, Boolean(picture));
  const strength = picture ? 0.38 : 0.85;
  const band = picture ? '60%' : '50%';

  const ready = (gl: ExpoWebGLRenderingContext) => {
    const extra: Record<string, Uniform> = picture && aspect ? { u_imageAspectRatio: aspect, u_image: 0 } : {};
    // Only the newest surface is on screen; one a remount left behind stops drawing.
    loops.current.forEach((loop) => loop.stop());
    loops.current.clear();
    const surface = mount(gl, fragment, { ...uniforms, ...extra });
    if (!surface) return;
    if (picture) {
      const texture = gl.createTexture();
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, texture);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
      const loading = uploadPicture(gl, picture);
      if (loading) loading.then(() => draw(surface, 0));
      else draw(surface, 0);
      return;
    }
    draw(surface, still ? STILL_MS * speed : (Date.now() - ORIGIN) * speed);
    // The loop holds this surface itself: a screen can mount more than one before settling on one.
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = () => {
      draw(surface, (Date.now() - ORIGIN) * speed);
      timer = setTimeout(tick, FRAME_MS);
    };
    const loop: Loop = {
      start: () => {
        if (timer === undefined) timer = setTimeout(tick, FRAME_MS);
      },
      stop: () => {
        clearTimeout(timer);
        timer = undefined;
      },
    };
    loops.current.add(loop);
    if (movingNow.current) loop.start();
  };

  return (
    <View pointerEvents="none" style={[styles.band, { height: band }]}>
      <GLView
        key={`${picture ?? 'grain'}:${colors.ground}:${colors.shaderDot}`}
        style={[StyleSheet.absoluteFill, { opacity: strength }]}
        onContextCreate={ready}
      />
      {/* The Mac masks the field out by the band's end; covering it with the ground does the same on an opaque screen. */}
      <Svg style={StyleSheet.absoluteFill} width="100%" height="100%">
        <Defs>
          <LinearGradient id="fade" x1="0" y1="0" x2="0" y2="1">
            <Stop offset="0" stopColor={colors.ground} stopOpacity={0} />
            <Stop offset="0.4" stopColor={colors.ground} stopOpacity={0.55} />
            <Stop offset="0.76" stopColor={colors.ground} stopOpacity={0.92} />
            <Stop offset="1" stopColor={colors.ground} stopOpacity={1} />
          </LinearGradient>
        </Defs>
        <Rect x="0" y="0" width="100%" height="100%" fill="url(#fade)" />
      </Svg>
    </View>
  );
}

const styles = StyleSheet.create({
  band: { position: 'absolute', left: 0, right: 0, top: 0, overflow: 'hidden' },
});
