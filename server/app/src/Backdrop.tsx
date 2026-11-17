import { useAuth } from "@clerk/react";
import { useEffect, useRef } from "react";

import { signedInToken } from "./api.ts";

/**
 * Behind a signed-in page, the backdrop of the host the person used last, read from it end to end;
 * otherwise, or while no host answers, the desktop app's grain.
 */
export function Backdrop() {
  const host = useRef<HTMLDivElement>(null);
  const { userId, getToken } = useAuth();

  useEffect(() => {
    const element = host.current;
    if (!element) return;
    let disposeGrain: (() => void) | undefined;
    let disposePicture: (() => void) | undefined;
    let cancelled = false;

    void showGrain(element).then((dispose) => {
      if (cancelled) dispose?.();
      else disposeGrain = dispose;
    });

    if (userId)
      void (async () => {
        const [{ readHostLook }, { drawPicture }] = await Promise.all([
          import("./look/hostLook.ts"),
          import("./look/drawPicture.ts"),
        ]);
        const reply = await readHostLook(
          userId,
          await signedInToken(getToken, "read your host's look"),
        );
        if (cancelled || reply?.result !== "look") return;
        if (!reply.texture) {
          disposeGrain?.();
          element.dataset.look = "none";
          return;
        }
        if (!reply.picture) return;
        const dispose = await drawPicture(element, reply.picture, reply.look);
        if (!dispose) return;
        if (cancelled) return dispose();
        disposePicture = dispose;
        element.dataset.look = reply.look.coverage;
        disposeGrain?.();
      })().catch(() => undefined);

    return () => {
      cancelled = true;
      disposeGrain?.();
      disposePicture?.();
      delete element.dataset.look;
    };
  }, [userId, getToken]);

  return <div ref={host} className="backdrop" aria-hidden="true" />;
}

/** The desktop app's ambient grain: Bayer dots over drifting noise, in Aura Noir's raised tone. */
async function showGrain(
  element: HTMLElement,
): Promise<(() => void) | undefined> {
  const {
    ShaderMount,
    ditheringFragmentShader,
    DitheringShapes,
    DitheringTypes,
    ShaderFitOptions,
    getShaderColorFromString,
  } = await import("@paper-design/shaders");
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
  try {
    const mount = new ShaderMount(
      element,
      ditheringFragmentShader,
      {
        u_colorBack: [0, 0, 0, 0],
        u_colorFront: getShaderColorFromString("#232329"),
        u_shape: DitheringShapes.simplex,
        u_type: DitheringTypes["8x8"],
        u_pxSize: 3,
        u_fit: ShaderFitOptions.none,
        u_scale: 2.4,
        u_rotation: 0,
        u_offsetX: 0,
        u_offsetY: 0,
        u_originX: 0.5,
        u_originY: 0.5,
        u_worldWidth: 0,
        u_worldHeight: 0,
      },
      { antialias: false },
      still ? 0 : 0.35,
    );
    element.dataset.ready = "";
    return () => mount.dispose();
  } catch {
    // Without WebGL the plain ground is the page, which is designed to stand alone.
    return undefined;
  }
}
