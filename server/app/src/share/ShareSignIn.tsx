import { useAuth } from "@clerk/react";
import { useEffect } from "react";

import { Backdrop } from "../Backdrop.tsx";
import { SignIn } from "../SignIn.tsx";
import "../app.css";

/**
 * Sign-in for a shared chat, in the account pages' own look. Once signed in, the page loads
 * again, so the chat is drawn without the account pages' styles beside the Mac's.
 */
export default function ShareSignIn() {
  const { isLoaded, isSignedIn } = useAuth();

  useEffect(() => {
    if (isSignedIn) location.reload();
  }, [isSignedIn]);

  return (
    <>
      <Backdrop />
      <div className="page">
        <main className="center">
          <p className="lede">
            Sign in to Sikemux to watch the chat shared with you.
          </p>
          <SignIn ready={isLoaded} />
        </main>
      </div>
    </>
  );
}
