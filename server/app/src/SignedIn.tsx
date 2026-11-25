import { useEffect } from "react";

import { useCheer } from "./Field.tsx";
import { Gate, GateActions } from "./Frame.tsx";
import { brandMark } from "./marks.ts";
import { btnClass } from "./ui.tsx";

export const SIGNED_IN = "/signed-in";

/** Where Sikemux on the computer sends the browser once it has the answer. */
export function SignedIn() {
  const cheer = useCheer();
  const cancelled = new URLSearchParams(location.search).has("cancelled");

  useEffect(() => {
    if (cancelled) return;
    const ringing = setTimeout(() => cheer(), 350);
    return () => clearTimeout(ringing);
  }, [cancelled, cheer]);

  return (
    <Gate
      title={cancelled ? "Sign-in cancelled" : "You're signed in"}
      lede={
        cancelled
          ? "Nothing changed. Try again from Sikemux."
          : "Sikemux on this computer is on your account."
      }
      mark={brandMark}
      scene="sign"
      tone={cancelled ? "failed" : "worked"}
      say="You can close this tab."
    >
      <GateActions>
        <a className={btnClass({ quiet: cancelled })} href="sikemux://">
          Back to Sikemux
        </a>
      </GateActions>
    </Gate>
  );
}
