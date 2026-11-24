import { useAuth } from "@clerk/react";
import { fromBase64Url, webCipher } from "@shared/share/cipher";
import { parsePageLink, type ShareLink } from "@shared/share/link";
import { lazy, Suspense, useEffect, useMemo, useState } from "react";

import { config } from "../config.ts";
import { DEFAULT_PALETTE, wearPicked } from "../chat/themes/bus.ts";
import { ReaderBar } from "./ReaderBar.tsx";
import "../chat/styles.css";
import "../chat/styles/chat/status.css";
import "../chat/styles/share.css";

const ShareSignIn = lazy(() => import("./ShareSignIn.tsx"));

/** What the host seals: the page's title and its whole HTML, pictures inlined. */
interface Sealed {
  title: string;
  html: string;
}

type Loaded =
  | { kind: "loading" }
  | { kind: "page"; page: Sealed }
  | { kind: "signIn" }
  | { kind: "gone" }
  | { kind: "failed" };

/**
 * The page's scripts run, but may fetch nothing: with no allow-same-origin the frame has an origin
 * of its own, so it cannot reach this page, its sign-in or the API either.
 */
const FRAME_POLICY = `<meta http-equiv="Content-Security-Policy" content="connect-src 'none'; form-action 'none'">`;

function framed(html: string): string {
  const doctype = /^\s*<!doctype[^>]*>/i.exec(html)?.[0] ?? "";
  return doctype + FRAME_POLICY + html.slice(doctype.length);
}

function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join(
    "",
  );
}

function isSealed(value: unknown): value is Sealed {
  const page = value as Partial<Sealed> | null;
  return typeof page?.title === "string" && typeof page.html === "string";
}

async function load(
  link: ShareLink,
  token: string | null,
): Promise<Exclude<Loaded, { kind: "loading" }>> {
  const cipher = webCipher(link);
  const proof = hex(fromBase64Url(await cipher.viewerProof()));
  const response = await fetch(`${config.apiUrl}/v1/pages/${link.id}`, {
    headers: {
      "x-page-proof": proof,
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
  });
  if (response.status === 401) return { kind: "signIn" };
  if (response.status === 404) return { kind: "gone" };
  if (!response.ok) return { kind: "failed" };
  const text = await cipher.open(await response.text());
  if (text === null) return { kind: "gone" };
  try {
    const page: unknown = JSON.parse(text);
    return isSealed(page) ? { kind: "page", page } : { kind: "gone" };
  } catch {
    return { kind: "gone" };
  }
}

/**
 * A page someone shared, at `/p/<id>#<key>`: their HTML in a sandboxed frame under one thin bar.
 * A page open to anyone is read without signing in; any other asks for a sign-in first. The key
 * after `#` never leaves the browser.
 */
export default function SharedPage() {
  const { isLoaded, getToken } = useAuth();
  const link = useMemo(() => parsePageLink(location.href), []);
  const [loaded, setLoaded] = useState<Loaded>({ kind: "loading" });

  useEffect(() => {
    wearPicked(DEFAULT_PALETTE);
  }, []);

  useEffect(() => {
    if (!link || !isLoaded) return;
    let current = true;
    void getToken()
      .then((token) => load(link, token))
      .catch((): Loaded => ({ kind: "failed" }))
      .then((next) => {
        if (current) setLoaded(next);
      });
    return () => {
      current = false;
    };
  }, [link, isLoaded, getToken]);

  const title = loaded.kind === "page" ? loaded.page.title.trim() : "";
  useEffect(() => {
    document.title = title ? `${title} · Sikemux` : "Shared artifact · Sikemux";
  }, [title]);

  if (link && loaded.kind === "signIn")
    return (
      <Suspense fallback={null}>
        <ShareSignIn lede="Sign in to Sikemux to read the artifact shared with you." />
      </Suspense>
    );

  return (
    <div className="shared-page">
      <ReaderBar>
        <span className="shared-page-title" title={title}>
          {title}
        </span>
      </ReaderBar>
      {loaded.kind === "page" ? (
        <iframe
          className="shared-page-frame"
          title={title || "Shared artifact"}
          sandbox="allow-scripts"
          srcDoc={framed(loaded.page.html)}
        />
      ) : (
        <PageState loaded={link ? loaded : { kind: "gone" }} />
      )}
    </div>
  );
}

function PageState({ loaded }: { loaded: Loaded }) {
  const loading = loaded.kind === "loading";
  return (
    <div
      className={`chat-connection-state ${loading ? "connecting" : "stopped"}`}
      role="status"
    >
      {loading && <span className="chat-activity-loader" aria-hidden="true" />}
      <span>
        {loading
          ? "Opening the shared artifact…"
          : loaded.kind === "failed"
            ? "The artifact didn’t load. Reload to try again."
            : "This artifact isn’t shared any more."}
      </span>
    </div>
  );
}
