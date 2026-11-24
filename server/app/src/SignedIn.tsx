import { drawDots } from "@shared/signin/dots.js";
import { IconCheck, IconClose } from "@shared/ui/Icons";
import { useLayoutEffect, useRef } from "react";

import mark from "../../../brand/mark/mark-purple.svg?raw";
import lockup from "../../../brand/wordmark/lockup-purple.svg";

export const SIGNED_IN = "/signed-in";

/** Where Sikemux on the computer sends the browser once it has the answer. */
export function SignedIn() {
  const canvas = useRef<HTMLCanvasElement>(null);
  const place = useRef<HTMLDivElement>(null);
  const cancelled = new URLSearchParams(location.search).has("cancelled");

  useLayoutEffect(() => {
    const page = place.current?.closest(".ending");
    if (!canvas.current || !place.current || !page) return;
    const { color } = getComputedStyle(place.current);
    const dots = drawDots(canvas.current, {
      mark: place.current,
      quiet: () => [
        ...page.querySelectorAll<HTMLElement>(
          "main > :not(.ending-mark), footer",
        ),
      ],
    });
    dots?.look({ tint: color, tone: cancelled ? "failed" : "worked" });
    dots?.show({ svg: mark, color });
    const ringing = cancelled
      ? undefined
      : setTimeout(() => dots?.ring(color), 350);
    return () => {
      clearTimeout(ringing);
      dots?.stop();
    };
  }, [cancelled]);

  return (
    <div className="ending" data-cancelled={cancelled || undefined}>
      <canvas ref={canvas} aria-hidden="true" />
      <main>
        <div className="ending-mark" aria-hidden="true">
          <div ref={place} dangerouslySetInnerHTML={{ __html: mark }} />
          <i>{cancelled ? <IconClose /> : <IconCheck />}</i>
        </div>
        <h1>{cancelled ? "Sign-in cancelled" : "You're signed in"}</h1>
        <p>
          {cancelled
            ? "Nothing changed. Try again from Sikemux."
            : "Sikemux on this computer is on your account."}
        </p>
        <a className="button primary" href="sikemux://">
          Back to Sikemux
        </a>
        <small>You can close this tab.</small>
      </main>
      <footer>
        <img src={lockup} alt="Sikemux" height="18" />
      </footer>
    </div>
  );
}
