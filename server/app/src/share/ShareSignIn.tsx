import { useAuth } from "@clerk/react";
import { useEffect } from "react";

import { Field } from "../Field.tsx";
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
    <Field>
      <SignIn ready={isLoaded} intro={{ title: "Sign in to Sikemux", lede }} />
    </Field>
  );
}
