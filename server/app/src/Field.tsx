import {
  createContext,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

import { drawDots, type Dots, type Picture, type Tone } from "./dots.ts";
import { sceneOf, wear, type SceneName } from "./scenes.ts";

export type Stage = {
  picture: Picture;
  scene: SceneName;
  tone?: Tone;
  hero?: boolean;
};

type FieldHandle = {
  spot: HTMLDivElement;
  stage: (stage: Stage) => void;
  cheer: () => void;
  measure: () => void;
};

const FieldContext = createContext<FieldHandle | null>(null);

/** Puts the picture itself in the spot, for a window without dots. */
function drawPlain(spot: HTMLElement, picture: Picture) {
  spot.innerHTML =
    "svg" in picture
      ? picture.svg
      : "image" in picture
        ? `<img src="${picture.image}" alt="">`
        : `<b>${picture.letter}</b>`;
}

/** One canvas of dots behind every page. It outlives them, so a page's mark turns into the next one's. */
export function Field({ children }: { children: ReactNode }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const dots = useRef<Dots>(null);
  const stage = useRef<Stage>(undefined);
  const [spot] = useState(() => {
    const element = document.createElement("div");
    element.className = "spot";
    return element;
  });

  const handle = useMemo<FieldHandle>(() => {
    const paint = ({ picture, scene, tone, hero }: Stage) => {
      const { tint, alt, deep, bright } = sceneOf(scene);
      dots.current?.look({
        tint,
        alt,
        deep,
        bright,
        glow: hero ? 0.16 : 0.24,
        tone: tone ?? "worked",
        reach: hero ? 0.58 : 0.75,
        rich: hero ? 0.9 : 0.8,
        clearing: 0.5,
      });
      dots.current?.show(picture);
      drawPlain(spot, picture);
    };
    return {
      spot,
      stage: (next) => {
        stage.current = next;
        wear(sceneOf(next.scene));
        paint(next);
        dots.current?.measure();
      },
      cheer: () =>
        dots.current?.ring(
          getComputedStyle(document.documentElement)
            .getPropertyValue("--ok")
            .trim(),
        ),
      measure: () => dots.current?.measure(),
    };
  }, [spot]);

  useEffect(() => {
    if (!canvas.current) return;
    const drawn = drawDots(canvas.current, {
      mark: spot,
      quiet: () => [...document.querySelectorAll<HTMLElement>("[data-quiet]")],
    });
    dots.current = drawn;
    if (stage.current) handle.stage(stage.current);
    return () => {
      dots.current = null;
      drawn?.stop();
    };
  }, [spot, handle]);

  return (
    <FieldContext value={handle}>
      <canvas className="dots" ref={canvas} aria-hidden="true" />
      {children}
    </FieldContext>
  );
}

function useField(): FieldHandle {
  const field = useContext(FieldContext);
  if (!field) throw new Error("No dots behind this page");
  return field;
}

/** Rings the mark once, for an action that went through. */
export function useCheer(): () => void {
  return useField().cheer;
}

/** Where the dots gather into the page's picture. */
export function Mark({ picture, scene, tone, hero }: Stage) {
  const field = useField();
  const box = useRef<HTMLDivElement>(null);
  const key = JSON.stringify(picture);
  useLayoutEffect(() => {
    box.current?.append(field.spot);
    const moves = new ResizeObserver(field.measure);
    for (const around of [
      box.current?.parentElement,
      box.current?.closest("main"),
    ])
      if (around) moves.observe(around);
    return () => {
      moves.disconnect();
      field.spot.remove();
    };
  }, [field]);
  useLayoutEffect(() => {
    field.stage({ picture: JSON.parse(key) as Picture, scene, tone, hero });
  }, [field, key, scene, tone, hero]);
  return <div className="markbox" ref={box} aria-hidden="true" />;
}
