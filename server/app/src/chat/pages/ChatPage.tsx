import { useContext, useEffect, useRef, useState } from "react";
import type { PageRef } from "@shared/wire.generated";
import { currentTheme, subscribeTheme } from "../themes/bus";
import { openLink, ReaderScrollContext } from "../transcript/chatAgent";
import { pageThemeMessage } from "./theme";
import {
  clampPageHeight,
  PAGE_DEFAULT_HEIGHT,
  readPageMessage,
} from "./messages";

/* A chat opened again mounts its pages afresh; a page coming back takes the
   height it last reported, so the rows around it do not move. */
const reportedHeights = new Map<string, number>();

/* The page's own script can ask for a link at any time, so only a click or key the
   person has just given the page opens one. Their click there activates this window too. */
function personJustActedIn(frame: HTMLIFrameElement): boolean {
  return document.activeElement === frame && navigator.userActivation.isActive;
}

/** A page an agent showed, drawn as part of its reply in a sandboxed frame. */
export function ChatPage({ page }: { page: PageRef }) {
  const scrollByReader = useContext(ReaderScrollContext);
  const frame = useRef<HTMLIFrameElement>(null);
  const [height, setHeight] = useState(() =>
    clampPageHeight(
      reportedHeights.get(page.id) ?? page.height ?? PAGE_DEFAULT_HEIGHT,
    ),
  );
  const [dark, setDark] = useState(() => currentTheme().dark);

  useEffect(() => {
    const receive = (event: MessageEvent) => {
      if (!frame.current || event.source !== frame.current.contentWindow)
        return;
      const message = readPageMessage(event.data);
      if (message?.kind === "height") {
        reportedHeights.set(page.id, message.height);
        setHeight(clampPageHeight(message.height));
      } else if (message?.kind === "link" && personJustActedIn(frame.current))
        openLink(message.url);
      else if (message?.kind === "wheel") scrollByReader(message.deltaY);
    };
    window.addEventListener("message", receive);
    const unsubscribe = subscribeTheme((theme) => {
      setDark(theme.dark);
      frame.current?.contentWindow?.postMessage(pageThemeMessage(theme), "*");
    });
    return () => {
      window.removeEventListener("message", receive);
      unsubscribe();
    };
  }, [page.id, scrollByReader]);

  return (
    <div className="chat-page" style={{ height }}>
      <iframe
        ref={frame}
        className="chat-page-frame"
        title={page.title}
        sandbox="allow-scripts allow-forms"
        style={{ colorScheme: dark ? "dark" : "light" }}
      />
    </div>
  );
}
