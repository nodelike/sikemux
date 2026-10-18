import { useAuth } from "@clerk/react";
import { lazy, Suspense, useEffect, useMemo } from "react";
import { webCipher } from "@mac/share/cipher";
import { appShareLink, parseShareLink, type ShareLink } from "@mac/share/link";
import { ShareSession } from "@mac/share/session";
import { SharedChat } from "@mac/share/SharedChat";
import { DEFAULT_THEME_ID } from "@mac/themes";
import { applyTheme } from "@mac/themes/bus";
import "@mac/styles.css";

import { config } from "../config.ts";

const ShareSignIn = lazy(() => import("./ShareSignIn.tsx"));

/** The share socket of the API this page talks to. */
const SOCKET = `${config.apiUrl.replace(/^http/, "ws")}/v1/share`;

/**
 * A chat someone shared, at `/s/<id>#<key>`: the Mac's own chat view, read-only, for anyone
 * signed in. The key after `#` never leaves the browser.
 */
export default function SharePage() {
  const { isLoaded, isSignedIn } = useAuth();
  const link = useMemo(() => parseShareLink(location.href), []);

  useEffect(() => {
    document.title = "Shared chat · Sikemux";
    applyTheme(DEFAULT_THEME_ID);
  }, []);

  if (!link)
    return (
      <div className="shell share-page">
        <div className="chat-connection-state stopped" role="status">
          <span>
            This link is missing part of itself. Ask for it again, and copy the
            whole of it.
          </span>
        </div>
      </div>
    );
  if (!isLoaded) return <div className="shell share-page" />;
  if (!isSignedIn)
    return (
      <Suspense fallback={null}>
        <ShareSignIn />
      </Suspense>
    );
  return <Watching link={link} />;
}

function Watching({ link }: { link: ShareLink }) {
  const { getToken } = useAuth();
  const session = useMemo(
    () =>
      new ShareSession({
        link,
        url: SOCKET,
        cipher: webCipher(link),
        token: (fresh) => getToken(fresh ? { skipCache: true } : undefined),
      }),
    [link, getToken],
  );
  useEffect(() => {
    session.start();
    return () => session.stop();
  }, [session]);

  return (
    <div className="shell share-page">
      <SharedChat
        session={session}
        visible
        actions={
          <div className="agent-view-switch">
            <button
              type="button"
              aria-pressed="false"
              onClick={() => location.assign(appShareLink(link))}
            >
              Open in Sikemux
            </button>
          </div>
        }
      />
    </div>
  );
}
