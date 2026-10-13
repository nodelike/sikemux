import { AGENTS } from "./world/workspace";

const tab = (id: string, url: string, title: string, active: boolean) => ({
  id,
  url,
  title,
  active,
  loading: false,
  canGoBack: true,
  canGoForward: false,
  favicon: null,
  acting: false,
});

export const BROWSER_TABS: Record<
  string,
  { tabs: ReturnType<typeof tab>[]; activeTabId: string }
> = {
  [AGENTS.hero.id]: {
    tabs: [
      tab(
        "tab-site",
        "http://localhost:5173/",
        "Maison Verre · Issue 07",
        true,
      ),
    ],
    activeTabId: "tab-site",
  },
};

const SNAPSHOTS: Record<string, string> = {
  [AGENTS.hero.id]: "/showcase/pages/maison-home.png",
};
const frames = new Map<string, HTMLImageElement>();

// The real browser is a native view laid over the pane, so its stand-in floats over the page the same way.
export function placeBrowserPage(
  agentId: string,
  bounds: DOMRectInit | null,
): void {
  let frame = frames.get(agentId);
  if (!bounds) {
    if (frame) frame.style.display = "none";
    return;
  }
  if (!frame) {
    frame = document.createElement("img");
    frame.src = SNAPSHOTS[agentId] ?? "";
    frame.alt = "";
    frame.dataset.showcaseBrowser = agentId;
    Object.assign(frame.style, {
      position: "fixed",
      zIndex: "5",
      objectFit: "cover",
      objectPosition: "top left",
      background: "#0d1117",
    });
    document.body.append(frame);
    frames.set(agentId, frame);
  }
  Object.assign(frame.style, {
    display: "block",
    left: `${bounds.x}px`,
    top: `${bounds.y}px`,
    width: `${bounds.width}px`,
    height: `${bounds.height}px`,
  });
}
