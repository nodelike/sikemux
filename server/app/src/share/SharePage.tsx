import { useAuth } from "@clerk/react";
import { shareUrl } from "@sikemux/protocol/live";
import { lazy, Suspense, useEffect, useMemo } from "react";
import { webCipher } from "@shared/share/cipher";
import { parseShareLink, type ShareLink } from "@shared/share/link";
import { ShareSession } from "@shared/share/session";
import { SharedChat } from "./SharedChat.tsx";
import { DEFAULT_PALETTE, wearPicked } from "../chat/themes/bus.ts";
import "../chat/styles.css";

import { config } from "../config.ts";

const ShareSignIn = lazy(() => import("./ShareSignIn.tsx"));

/**
 * A chat someone shared, at `/s/<id>#<key>`: the app's chat view, read-only, for anyone
 * signed in. The key after `#` never leaves the browser.
 */
export default function SharePage() {
  const { isLoaded, isSignedIn } = useAuth();
  const link = useMemo(() => parseShareLink(location.href), []);

  useEffect(() => {
    document.title = "Shared chat · Sikemux";
    wearPicked(DEFAULT_PALETTE);
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
        url: shareUrl(config.apiUrl),
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
      <SharedChat session={session} link={link} />
    </div>
  );
}
