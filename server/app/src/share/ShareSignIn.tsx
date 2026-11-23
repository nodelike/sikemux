import { useAuth } from "@clerk/react";
import { useEffect } from "react";

import { Backdrop } from "../Backdrop.tsx";
import { SignIn } from "../SignIn.tsx";
import "../app.css";

/**
 * Sign-in for a shared chat or page, in the account pages' own look. Once signed in, the page
 * loads again, so what was shared is drawn without the account pages' styles beside the Mac's.
 */
export default function ShareSignIn({
  lede = "Sign in to Sikemux to watch the chat shared with you.",
}: {
  lede?: string;
}) {
  const { isLoaded, isSignedIn } = useAuth();

  useEffect(() => {
    if (isSignedIn) location.reload();
  }, [isSignedIn]);

  return (
    <>
      <Backdrop />
      <div className="page">
        <main className="center">
          <p className="lede">{lede}</p>
          <SignIn ready={isLoaded} />
        </main>
      </div>
    </>
  );
}
