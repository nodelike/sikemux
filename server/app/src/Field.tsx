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
  hot: (over: { picture: Picture; scene?: SceneName } | null) => void;
  cheer: () => void;
  measure: () => void;
};

const FieldContext = createContext<FieldHandle | null>(null);

const LEAVE_MS = 380;

/** Puts the picture itself in the spot, for a window without dots. */
function drawPlain(spot: HTMLElement, picture: Picture) {
  spot.innerHTML = "svg" in picture ? picture.svg : `<b>${picture.letter}</b>`;
}

/** One canvas of dots behind every page. It outlives them, so a page's mark turns into the next one's. */
export function Field({ children }: { children: ReactNode }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const dots = useRef<Dots>(null);
  const stage = useRef<Stage>(undefined);
  const leaving = useRef<ReturnType<typeof setTimeout>>(undefined);
  const [spot] = useState(() => {
    const element = document.createElement("div");
    element.className = "spot";
    return element;
  });

  const handle = useMemo<FieldHandle>(() => {
    const paint = (picture: Picture, name: SceneName) => {
      const { tint, alt, deep, bright } = sceneOf(name);
      const on = stage.current;
      dots.current?.look({
        tint,
        alt,
        deep,
        bright,
        glow: on?.hero ? 0.16 : 0.3,
        tone: on?.tone ?? "worked",
        reach: on?.hero ? 0.62 : 0.85,
        rich: on?.hero ? 1 : 0.92,
        clearing: 0.5,
      });
      dots.current?.show(picture);
      drawPlain(spot, picture);
    };
    const settle = () => {
      const on = stage.current;
      if (on) paint(on.picture, on.scene);
    };
    return {
      spot,
      stage: (next) => {
        clearTimeout(leaving.current);
        stage.current = next;
        wear(sceneOf(next.scene));
        settle();
        dots.current?.measure();
      },
      hot: (over) => {
        clearTimeout(leaving.current);
        const on = stage.current;
        if (!over) leaving.current = setTimeout(settle, LEAVE_MS);
        else if (on) paint(over.picture, over.scene ?? on.scene);
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
      quiet: () =>
        [...document.querySelectorAll<HTMLElement>("[data-quiet]")].slice(0, 6),
    });
    dots.current = drawn;
    if (stage.current) handle.stage(stage.current);
    return () => {
      clearTimeout(leaving.current);
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

/** Props for a row or card whose own picture, if it has one, the dots show while the pointer is over it. */
export function useHot(picture: Picture | undefined, scene?: SceneName) {
  const field = useField();
  if (!picture) return { "data-calm": "" };
  return {
    "data-calm": "",
    onPointerEnter: () => field.hot({ picture, scene }),
    onPointerLeave: () => field.hot(null),
  };
}
