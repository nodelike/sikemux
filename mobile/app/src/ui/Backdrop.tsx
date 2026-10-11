import { createContext, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { Animated, Image, PixelRatio, StyleSheet, useWindowDimensions, View } from 'react-native';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';
import { GLView, type ExpoWebGLRenderingContext } from 'expo-gl';
import { File, Paths } from 'expo-file-system';
import { useIsFocused } from 'expo-router';
import { ditheringFragmentShader, getShaderColorFromString, imageDitheringFragmentShader } from '@paper-design/shaders';

import type { DeviceBackdrop } from '@/devices/backdrop';
import { useAppActive, useBackdropPaused, useStill } from './motion';
import { useColors, type Palette } from './theme';
import { uploadPicture } from './picture';
import { vertexShaderSource } from './vertexShader.generated';

/** What the host a screen belongs to draws behind its panes; absent outside a device's screens. */
export const BackdropContext = createContext<DeviceBackdrop | undefined>(undefined);

/** Every backdrop reads one clock, so the grain carries on across screens instead of restarting. */
const ORIGIN = Date.now();
/** The Mac ticks its grain at 30 fps, 20 on battery; a phone draws it on its JavaScript thread, so slower still. */
const FRAME_MS = 80;
const FADE_MS = 400;
/** Where the grain rests when motion is reduced, as on the Mac. */
const STILL_MS = 2500;

/** Each picture's shape, measured once, and the backdrops already shown, so a screen opened later draws at once. */
const aspects = new Map<string, number>();
const revealed = new Set<string>();

/* A picture backdrop never moves, so its first drawn frame is kept as an image, across launches,
   and every later screen shows that instead of drawing it again. */
function keptFrame(key: string): File {
  let hash = 5381;
  for (const char of key) hash = ((hash * 33) ^ char.charCodeAt(0)) >>> 0;
  return new File(Paths.cache, `backdrop-${hash.toString(16)}.png`);
}

async function keepFrame(gl: ExpoWebGLRenderingContext, key: string): Promise<string> {
  const shot = await GLView.takeSnapshotAsync(gl, { format: 'png' });
  const kept = keptFrame(key);
  new File(shot.localUri).move(kept, { overwrite: true });
  return kept.uri;
}

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
  resolution: WebGLUniformLocation | null;
  size: [number, number];
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
  return {
    gl,
    program,
    buffer,
    position,
    time: gl.getUniformLocation(program, 'u_time'),
    resolution: gl.getUniformLocation(program, 'u_resolution'),
    size: [gl.drawingBufferWidth, gl.drawingBufferHeight],
  };
}

/** expo-gl presents a frame with GL calls of its own, so each frame binds everything again. */
function draw(surface: Surface, frameMs: number) {
  const { gl } = surface;
  gl.useProgram(surface.program);
  gl.bindBuffer(gl.ARRAY_BUFFER, surface.buffer);
  gl.enableVertexAttribArray(surface.position);
  gl.vertexAttribPointer(surface.position, 2, gl.FLOAT, false, 0, 0);
  gl.viewport(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight);
  if (surface.size[0] !== gl.drawingBufferWidth || surface.size[1] !== gl.drawingBufferHeight) {
    surface.size = [gl.drawingBufferWidth, gl.drawingBufferHeight];
    if (surface.resolution) gl.uniform2fv(surface.resolution, surface.size);
  }
  gl.clearColor(0, 0, 0, 0);
  gl.enable(gl.BLEND);
  gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
  if (surface.time) gl.uniform1f(surface.time, frameMs * 1e-3);
  gl.clear(gl.COLOR_BUFFER_BIT);
  gl.drawArrays(gl.TRIANGLES, 0, 6);
  gl.flush();
  gl.endFrameEXP();
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
  const paused = useBackdropPaused();
  const still = useStill();
  const picture = backdrop?.texture ? backdrop.image : undefined;
  const window = useWindowDimensions();
  const frameKey = backdrop?.texture
    ? [
        picture ? picture.slice(picture.lastIndexOf('/') + 1) : 'grain',
        colors.ground,
        colors.shaderDot,
        Math.round(window.width),
        Math.round(window.height),
      ].join('|')
    : undefined;
  const [justKept, setJustKept] = useState<{ key: string; uri: string }>();
  const onDisk = useMemo(() => {
    if (!frameKey) return undefined;
    const kept = keptFrame(frameKey);
    return kept.exists ? kept.uri : undefined;
  }, [frameKey]);
  // Once this screen's frame is kept, the image takes the GL surface's place: an idle surface still wakes every frame.
  const frame = onDisk ?? (justKept && justKept.key === frameKey ? justKept.uri : undefined);
  const [measured, setMeasured] = useState<{ picture: string; aspect: number }>();
  const aspect = picture ? (aspects.get(picture) ?? (measured?.picture === picture ? measured.aspect : undefined)) : undefined;
  const moving = !picture && focused && active && !paused && !still;
  // The grain animates only on the screen showing; anywhere else its kept frame stands in, with no GL surface.
  const animating = !picture && focused && active && !still;
  const drawing = !frame || animating;
  const loops = useRef(new Set<Loop>());
  const movingNow = useRef(moving);
  const [shown] = useState(() => new Animated.Value(frame ? 1 : 0));
  const show = () => {
    const key = picture ?? 'grain';
    if (still || revealed.has(key)) shown.setValue(1);
    else Animated.timing(shown, { toValue: 1, duration: FADE_MS, useNativeDriver: true }).start();
    revealed.add(key);
  };

  useEffect(() => {
    if (!picture || aspects.has(picture)) return;
    Image.getSize(
      picture,
      (width, height) => {
        aspects.set(picture, width / height);
        setMeasured({ picture, aspect: width / height });
      },
      () => {},
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

  if (!backdrop?.texture || (picture && !aspect && !frame)) return null;
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
      const reveal = () => {
        draw(surface, 0);
        show();
        if (frameKey) {
          const key = frameKey;
          keepFrame(gl, key)
            .then((uri) => setJustKept({ key, uri }))
            .catch(() => {});
        }
      };
      if (loading) loading.then(reveal).catch(() => {});
      else reveal();
      return;
    }
    draw(surface, still ? STILL_MS * speed : (Date.now() - ORIGIN) * speed);
    show();
    if (frameKey && !frame) {
      const key = frameKey;
      keepFrame(gl, key)
        .then((uri) => setJustKept({ key, uri }))
        .catch(() => {});
    }
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
      <Animated.View style={[StyleSheet.absoluteFill, { opacity: Animated.multiply(shown, strength) }]}>
        {frame ? <Image source={{ uri: frame }} style={StyleSheet.absoluteFill} resizeMode="stretch" /> : null}
        {drawing ? (
          <GLView
            key={`${picture ?? 'grain'}:${colors.ground}:${colors.shaderDot}`}
            style={StyleSheet.absoluteFill}
            msaaSamples={0}
            onContextCreate={ready}
          />
        ) : null}
      </Animated.View>
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
