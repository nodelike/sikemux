import { useContext, useEffect, useState } from "react";
import { isImagePath } from "@shared/lib/imagePaths";
import { RemoteOriginContext, type RemoteOrigin } from "../transcript/origin";

/* Pictures from another computer, by where they came from, so two shared chats never mix theirs. */
const remotePreviews = new WeakMap<RemoteOrigin, Map<string, string | null>>();

/** A picture pasted into a shared chat, as its host sends it; null while it loads and if the host will not send it. */
export function useImagePreview(path: string | null): string | null {
  const origin = useContext(RemoteOriginContext);
  const image = path && isImagePath(path) ? path : null;
  const held = image ? remotePreviews.get(origin)?.get(image) : undefined;
  const [loaded, setLoaded] = useState<{
    path: string;
    src: string | null;
  } | null>(null);
  useEffect(() => {
    if (!image || held !== undefined) return;
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
