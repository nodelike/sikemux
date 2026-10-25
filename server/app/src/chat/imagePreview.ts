import { useEffect, useState } from "react";
import { fsapi } from "./page/files";
import { previewUrl } from "./page/files";
import { isImagePath } from "@shared/lib/imagePaths";
import { useRemoteOrigin, type RemoteOrigin } from "./origin";

const MAX_CACHED = 12;
/* A preview is only ever drawn a few hundred pixels wide, so a picture bigger
   than this is redrawn small before it is kept. A retina screenshot is several
   megabytes and would otherwise fill the cache on its own. */
const SHRINK_OVER_BYTES = 1024 * 1024;
const THUMB_EDGE = 720;
const MAX_CACHE_BYTES = 12 * 1024 * 1024;
/* A null entry is a file we already tried and cannot show, so a transcript
   that scrolls past it again does not read it again. */
const previews = new Map<string, string | null>();
const pending = new Map<string, Promise<string | null>>();
let cachedBytes = 0;

function remember(path: string, src: string | null): string | null {
  const held = previews.get(path);
  if (held !== undefined) cachedBytes -= held?.length ?? 0;
  previews.delete(path);
  previews.set(path, src);
  cachedBytes += src?.length ?? 0;
  for (const oldest of [...previews.keys()]) {
    if (
      oldest === path ||
      (previews.size <= MAX_CACHED && cachedBytes <= MAX_CACHE_BYTES)
    )
      break;
    cachedBytes -= previews.get(oldest)?.length ?? 0;
    previews.delete(oldest);
  }
  return src;
}

/** Redraws a picture at thumbnail size, or null where the webview cannot. */
async function shrink(blob: Blob): Promise<string | null> {
  if (typeof createImageBitmap !== "function") return null;
  try {
    const bitmap = await createImageBitmap(blob);
    const scale = Math.min(
      1,
      THUMB_EDGE / Math.max(bitmap.width, bitmap.height),
    );
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(bitmap.width * scale));
    canvas.height = Math.max(1, Math.round(bitmap.height * scale));
    const context = canvas.getContext("2d");
    if (context) context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    bitmap.close();
    return context ? canvas.toDataURL("image/jpeg", 0.82) : null;
  } catch {
    return null;
  }
}

/* An SVG that gives only a viewBox has no size of its own, and WebKit draws it
   0px wide anywhere the box it sits in sizes itself to its content. */
export function sizedSvg(markup: string): string {
  const document = new DOMParser().parseFromString(markup, "image/svg+xml");
  const root = document.documentElement;
  if (
    root.localName !== "svg" ||
    root.hasAttribute("width") ||
    root.hasAttribute("height")
  )
    return markup;
  const [, , width, height] = (root.getAttribute("viewBox") ?? "")
    .trim()
    .split(/[\s,]+/)
    .map(Number);
  if (!(width > 0 && height > 0)) return markup;
  root.setAttribute("width", String(width));
  root.setAttribute("height", String(height));
  return new XMLSerializer().serializeToString(document);
}

function svgSource(markup: string): string {
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(sizedSvg(markup))}`;
}

/* Small pictures are shown straight from disk; only an SVG needing a size and
   a picture big enough to shrink are ever read into the page. */
async function readSource(
  path: string,
  shrinkLarge: boolean,
): Promise<string | null> {
  const { mime, size } = await fsapi.previewFile(path);
  if (!mime.startsWith("image/")) return null;
  const url = previewUrl(path);
  if (mime === "image/svg+xml")
    return svgSource(await (await fetch(url)).text());
  if (!shrinkLarge || size <= SHRINK_OVER_BYTES) return url;
  return shrink(await (await fetch(url)).blob());
}

function loadPreview(path: string): Promise<string | null> {
  const running = pending.get(path);
  if (running) return running;
  const request = readSource(path, true)
    .then((src) => remember(path, src))
    .catch(() => remember(path, null))
    .finally(() => pending.delete(path));
  pending.set(path, request);
  return request;
}

/** What the preview cache is holding, in characters. */
export function previewCacheBytes(): number {
  return cachedBytes;
}

/** Reads a local image whole, for a viewer that wants it at its own size. */
export async function readImageSource(path: string): Promise<string | null> {
  try {
    return await readSource(path, false);
  } catch {
    return null;
  }
}

/* Pictures from another computer, by where they came from, so two shared chats never mix theirs. */
const remotePreviews = new WeakMap<RemoteOrigin, Map<string, string | null>>();

function useRemotePreview(
  origin: RemoteOrigin | null,
  image: string | null,
): string | null {
  const held =
    origin && image ? remotePreviews.get(origin)?.get(image) : undefined;
  const [loaded, setLoaded] = useState<{
    path: string;
    src: string | null;
  } | null>(null);
  useEffect(() => {
    if (!origin || !image || held !== undefined) return;
    let live = true;
    void origin.readImage(image).then((src) => {
      const kept =
        remotePreviews.get(origin) ?? new Map<string, string | null>();
      kept.set(image, src);
      remotePreviews.set(origin, kept);
      if (live) setLoaded({ path: image, src });
    });
    return () => {
      live = false;
    };
  }, [origin, image, held]);
  if (!image) return null;
  return held ?? (loaded?.path === image ? loaded.src : null);
}

/** Reads a local image as a data URL; null while it loads and if it cannot be shown. */
export function useImagePreview(path: string | null): string | null {
  const origin = useRemoteOrigin();
  const image = path && isImagePath(path) ? path : null;
  const remote = useRemotePreview(origin, image);
  /* Read from the cache on the first render, so a row the transcript mounts
       again is drawn at its full height straight away rather than growing. */
  const [loaded, setLoaded] = useState(() =>
    image && previews.has(image)
      ? { path: image, src: previews.get(image) ?? null }
      : null,
  );
  useEffect(() => {
    if (!image || origin) return;
    if (previews.has(image)) {
      const src = previews.get(image) ?? null;
      setLoaded((current) =>
        current?.path === image && current.src === src
          ? current
          : { path: image, src },
      );
      return;
    }
    let live = true;
    void loadPreview(image).then((src) => {
      if (live) setLoaded({ path: image, src });
    });
    return () => {
      live = false;
    };
  }, [image, origin]);
  if (!image) return null;
  if (origin) return remote;
  return loaded?.path === image ? loaded.src : (previews.get(image) ?? null);
}
