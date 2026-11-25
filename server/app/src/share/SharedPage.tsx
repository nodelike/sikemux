import { useAuth } from "@clerk/react";
import { fromBase64Url, webCipher } from "@shared/share/cipher";
import { parsePageLink, type ShareLink } from "@shared/share/link";
import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";

import { config } from "../config.ts";
import { DEFAULT_PALETTE, wearPicked } from "../chat/themes/bus.ts";
import { ReaderBar } from "./ReaderBar.tsx";
import { homeAction, ShareGate } from "./ShareGate.tsx";
import "../chat/styles.css";
import "../chat/styles/share.css";

const ShareSignIn = lazy(() => import("./ShareSignIn.tsx"));
const SCROLL_MESSAGE = "sikemux-page-scroll";

/** What the host seals: the page's title and its whole HTML, pictures inlined. */
interface Sealed {
  title: string;
  html: string;
}

interface ScrollPos {
  x: number;
  y: number;
}

type Loaded =
  | { kind: "loading" }
  | { kind: "page"; page: Sealed; srcDoc: string }
  | { kind: "signIn" }
  | { kind: "gone" }
  | { kind: "failed" };

type FetchResult =
  | { kind: "unchanged" }
  | { kind: "page"; page: Sealed; etag: string | null }
  | { kind: "signIn" }
  | { kind: "gone" }
  | { kind: "failed" };

/**
 * The page's scripts run, but may fetch nothing: with no allow-same-origin the frame has an origin
 * of its own, so it cannot reach this page, its sign-in or the API either.
 */
const FRAME_POLICY = `<meta http-equiv="Content-Security-Policy" content="connect-src 'none'; form-action 'none'">`;

function scrollBridge(scroll: ScrollPos): string {
  const x = Math.max(0, Math.round(scroll.x));
  const y = Math.max(0, Math.round(scroll.y));
  return `<script>(function(){var sx=${x},sy=${y},moved=false,raf=0;function send(){raf=0;try{parent.postMessage({type:"${SCROLL_MESSAGE}",x:Math.round(scrollX),y:Math.round(scrollY)},"*")}catch(e){}}addEventListener("scroll",function(){moved=true;if(!raf)raf=requestAnimationFrame(send)},{passive:true});if(sx||sy){function restore(){if(!moved)scrollTo(sx,sy)}addEventListener("DOMContentLoaded",restore,{once:true});addEventListener("load",restore,{once:true})}})();</script>`;
}

function framed(html: string, scroll: ScrollPos): string {
  const doctype = /^\s*<!doctype[^>]*>/i.exec(html)?.[0] ?? "";
  return (
    doctype + FRAME_POLICY + scrollBridge(scroll) + html.slice(doctype.length)
  );
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

async function fetchPage(
  link: ShareLink,
  token: string | null,
  ifNoneMatch: string | null,
): Promise<FetchResult> {
  const cipher = webCipher(link);
  const proof = hex(fromBase64Url(await cipher.viewerProof()));
  const response = await fetch(`${config.apiUrl}/v1/pages/${link.id}`, {
    cache: "no-store",
    headers: {
      "x-page-proof": proof,
      ...(token ? { authorization: `Bearer ${token}` } : {}),
      ...(ifNoneMatch ? { "if-none-match": ifNoneMatch } : {}),
    },
  });
  if (response.status === 304) return { kind: "unchanged" };
  if (response.status === 401) return { kind: "signIn" };
  if (response.status === 404) return { kind: "gone" };
  if (!response.ok) return { kind: "failed" };
  const text = await cipher.open(await response.text());
  if (text === null) return { kind: "gone" };
  try {
    const page: unknown = JSON.parse(text);
    return isSealed(page)
      ? { kind: "page", page, etag: response.headers.get("etag") }
      : { kind: "gone" };
  } catch {
    return { kind: "gone" };
  }
}

/**
 * A page someone shared, at `/p/<id>#<key>`: their HTML in a sandboxed frame under one thin bar.
 * A page open to anyone is read without signing in; any other asks for a sign-in first. The key
 * after `#` never leaves the browser. Returning to the tab checks for a newer version with ETag.
 */
export default function SharedPage() {
  const { isLoaded, isSignedIn, getToken } = useAuth();
  const link = useMemo(() => parsePageLink(location.href), []);
  const [loaded, setLoaded] = useState<Loaded>({ kind: "loading" });
  const iframeRef = useRef<HTMLIFrameElement | null>(null);
  const etagRef = useRef<string | null>(null);
  const scrollRef = useRef<ScrollPos>({ x: 0, y: 0 });
  const checkingRef = useRef(false);

  useEffect(() => {
    wearPicked(DEFAULT_PALETTE);
  }, []);

  useEffect(() => {
    function onMessage(event: MessageEvent) {
      if (event.source !== iframeRef.current?.contentWindow) return;
      const data = event.data as {
        type?: unknown;
        x?: unknown;
        y?: unknown;
      } | null;
      if (
        data?.type !== SCROLL_MESSAGE ||
        typeof data.x !== "number" ||
        typeof data.y !== "number" ||
        !Number.isFinite(data.x) ||
        !Number.isFinite(data.y)
      )
        return;
      scrollRef.current = {
        x: Math.max(0, Math.round(data.x)),
        y: Math.max(0, Math.round(data.y)),
      };
    }
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, []);

  useEffect(() => {
    if (!link || !isLoaded) return;
    let current = true;
    etagRef.current = null;
    scrollRef.current = { x: 0, y: 0 };
    void getToken()
      .then((token) => fetchPage(link, token, null))
      .catch((): FetchResult => ({ kind: "failed" }))
      .then((result) => {
        if (!current) return;
        if (result.kind === "page") {
          etagRef.current = result.etag;
          setLoaded({
            kind: "page",
            page: result.page,
            srcDoc: framed(result.page.html, { x: 0, y: 0 }),
          });
        } else if (result.kind !== "unchanged") {
          setLoaded(result);
        }
      });
    return () => {
      current = false;
    };
  }, [link, isLoaded, getToken]);

  useEffect(() => {
    if (!link || !isLoaded) return;
    const shareLink = link;
    let active = true;
    function refresh() {
      if (
        document.visibilityState === "hidden" ||
        checkingRef.current ||
        !etagRef.current
      )
        return;
      checkingRef.current = true;
      void getToken()
        .then((token) => fetchPage(shareLink, token, etagRef.current))
        .catch((): FetchResult => ({ kind: "failed" }))
        .then((result) => {
          if (!active) return;
          if (result.kind === "page") {
            etagRef.current = result.etag;
            setLoaded({
              kind: "page",
              page: result.page,
              srcDoc: framed(result.page.html, scrollRef.current),
            });
          } else if (result.kind === "gone" || result.kind === "signIn") {
            etagRef.current = null;
            setLoaded(result);
          }
        })
        .finally(() => {
          checkingRef.current = false;
        });
    }
    function onVisibility() {
      if (document.visibilityState === "visible") refresh();
    }
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      active = false;
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [link, isLoaded, getToken]);

  const title = loaded.kind === "page" ? loaded.page.title.trim() : "";
  useEffect(() => {
    document.title = title ? `${title} · Sikemux` : "Shared artifact · Sikemux";
  }, [title]);

  if (!link)
    return (
      <ShareGate
        title="This link is missing part of itself"
        lede="Ask whoever shared it to send the whole link, including the part after #."
        icon="link"
        tone="failed"
        action={homeAction(Boolean(isSignedIn))}
      />
    );

  if (loaded.kind === "signIn")
    return (
      <Suspense fallback={null}>
        <ShareSignIn lede="Sign in to read the artifact shared with you." />
      </Suspense>
    );

  if (loaded.kind === "loading")
    return (
      <ShareGate
        title="Opening the shared artifact…"
        icon="file"
        tone="waiting"
        foot={false}
      />
    );

  if (loaded.kind === "failed")
    return (
      <ShareGate
        title="The artifact didn’t load"
        lede="Something went wrong fetching it. Reload the page, or ask whoever shared it if this keeps happening."
        icon="file"
        tone="failed"
        action={{ label: "Reload", onClick: () => location.reload() }}
      />
    );

  if (loaded.kind === "gone")
    return (
      <ShareGate
        title="This artifact isn’t shared any more"
        lede="The sharer stopped sharing it or the link expired. Ask whoever shared it for a new link."
        icon="file"
        tone="waiting"
        action={homeAction(Boolean(isSignedIn))}
      />
    );

  return (
    <div className="shared-page">
      <ReaderBar title={title} />
      <iframe
        ref={iframeRef}
        className="shared-page-frame"
        title={title || "Shared artifact"}
        sandbox="allow-scripts"
        srcDoc={loaded.srcDoc}
      />
    </div>
  );
}
