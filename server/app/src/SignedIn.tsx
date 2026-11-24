import { drawDots } from "@shared/signin/dots.js";
import { IconCheck, IconClose } from "@shared/ui/Icons";
import { useEffect, useRef } from "react";

import mark from "../../../brand/mark/mark-purple.svg?raw";
import lockup from "../../../brand/wordmark/lockup-purple.svg";

export const SIGNED_IN = "/signed-in";

/** Where Sikemux on the computer sends the browser once it has the answer. */
export function SignedIn() {
  const canvas = useRef<HTMLCanvasElement>(null);
  const place = useRef<HTMLDivElement>(null);
  const cancelled = new URLSearchParams(location.search).has("cancelled");

  useEffect(() => {
    if (!canvas.current || !place.current) return;
    return drawDots(canvas.current, place.current, {
      tint: getComputedStyle(place.current).color,
      failed: cancelled,
    });
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
