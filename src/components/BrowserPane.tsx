import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { browserApi, BLANK_URL, type BrowserBounds, type BrowserSnapshot } from "../api/browser";
import { onStageFrame, useNativeViewsOccluded, useStageMoving } from "../state/nativeViews";
import type { AgentType } from "../state/types";
import { reportError } from "../state/toast";
import { AgentIcon, IconChevron, IconGlobe, IconPlus, IconRefresh } from "./Icons";
import { TabBar } from "./TabBar";
import { useStore } from "../state/store";
import { EMPTY_STRIP, refreshBrowserStrip, takeBrowserRestore } from "../state/browserStrips";

/*
 * Which scrollers can move this pane on screen: its own scrolling ancestors,
 * and the window. Listening on the window in the capture phase instead meant
 * every scroll anywhere in the app — a chat transcript, a terminal, a file tree
 * — asked the browser pane to re-measure itself.
 */
function scrollParents(element: HTMLElement): (HTMLElement | Window)[] {
    const parents: (HTMLElement | Window)[] = [window];
    for (let node = element.parentElement; node; node = node.parentElement) {
        const style = getComputedStyle(node);
        if (/auto|scroll|overlay/.test(`${style.overflowX} ${style.overflowY}`)) parents.push(node);
    }
    return parents;
}

function sameBounds(a: BrowserBounds | null, b: BrowserBounds): boolean {
    return !!a && a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;
}

/**
 * A browser pane, as an ordinary leaf in the window layout.
 *
 * It is a sibling of the agent it belongs to rather than something drawn
 * inside it, so it is split, resized, focused and closed by the same layout
 * the terminals use. `browserPanes` is what ties it back to its agent.
 */
export function BrowserPaneHost({ paneId, visible, painted, onEmpty }: { paneId: string; visible: boolean; painted: boolean; onEmpty: () => void }) {
    const agentId = useStore((state) => state.browserPanes[paneId]);
    const agentType = useStore((state) => (agentId ? state.agents[agentId]?.type : undefined));
    /* Restored from a layout whose agent is gone — the association is the only
       thing that made this pane mean anything, so it closes. */
    const orphaned = !agentId || !agentType;
    useEffect(() => {
        if (orphaned) onEmpty();
    }, [onEmpty, orphaned]);
    if (orphaned) return null;
    return (
        <BrowserSession key={agentId} paneId={paneId} agentId={agentId} agentType={agentType} visible={visible} painted={painted} onEmpty={onEmpty} />
    );
}

/** The site's own mark once it has arrived, and a globe until then. */
function SiteIcon({ src }: { src: string | null }) {
    const [broken, setBroken] = useState(false);
    useEffect(() => setBroken(false), [src]);
    if (!src || broken) return <IconGlobe size={13} />;
    return <img className="tab-favicon" src={src} alt="" onError={() => setBroken(true)} />;
}

function BrowserSession({
    paneId,
    agentId,
    agentType,
    visible,
    painted,
    onEmpty,
}: {
    paneId: string;
    agentId: string;
    agentType: AgentType;
    visible: boolean;
    painted: boolean;
    onEmpty: () => void;
}) {
    const snapshot = useStore((state) => state.browserStrips[agentId]) ?? EMPTY_STRIP;
    const restoring = useStore((state) => !!state.browserRestores[paneId]);

    const refresh = useCallback(async () => {
        await refreshBrowserStrip(agentId);
    }, [agentId]);

    /* The app keeps the strips up to date for every pane; this one only has to
       ask for the first read, since its agent may have had no browser at all
       until the click that opened this pane. */
    useEffect(() => {
        if (!visible) return;
        void refresh().catch((error) => console.warn("browser session read failed", error));
    }, [refresh, visible]);

    /*
     * Tabs saved by the last run wait here until someone looks at the pane, so
     * a restart does not spend a page on every browser that was left open.
     */
    useEffect(() => {
        if (!visible || !restoring) return;
        const saved = takeBrowserRestore(paneId);
        if (!saved) return;
        void (async () => {
            const opened: string[] = [];
            for (const tab of saved.tabs) opened.push(await browserApi.newTab(agentId, tab.url));
            const active = opened[saved.activeIndex];
            if (active) await browserApi.switchTab(agentId, active);
            await refresh();
        })().catch((error) => {
            reportError("restore browser tabs")(error);
            onEmpty();
        });
    }, [agentId, onEmpty, paneId, refresh, restoring, visible]);

    /*
     * The pane exists because a tab does, so when the last one goes it has
     * nothing left to show and closes itself.
     *
     * It has to have held one first. The pane is opened by the same click that
     * asks for the tab, and the tab arrives a round trip later — closing on an
     * empty snapshot alone would shut the pane before its first tab landed.
     */
    const heldATab = useRef(false);
    if (snapshot.tabs.length > 0) heldATab.current = true;
    useEffect(() => {
        if (!visible || restoring || !heldATab.current || snapshot.tabs.length > 0) return;
        onEmpty();
    }, [onEmpty, restoring, snapshot.tabs.length, visible]);

    return <BrowserPane agentId={agentId} agentType={agentType} visible={visible} painted={painted} snapshot={snapshot} refresh={refresh} />;
}

/* The page itself is a native view the window draws over this pane, so the
   pane's only job for it is to say where the page area is. */
function BrowserPane({
    agentId,
    agentType,
    visible,
    painted,
    snapshot,
    refresh,
}: {
    agentId: string;
    agentType: AgentType;
    visible: boolean;
    painted: boolean;
    snapshot: BrowserSnapshot;
    refresh: (signal?: AbortSignal) => Promise<void>;
}) {
    const viewportRef = useRef<HTMLDivElement>(null);
    const measureRef = useRef<() => void>(() => {});
    const [typed, setTyped] = useState<string | null>(null);
    const [placement, setPlacement] = useState<BrowserBounds | null>(null);
    const occluded = useNativeViewsOccluded();
    const moving = useStageMoving();
    const activeTab = useMemo(() => snapshot.tabs.find((tab) => tab.id === snapshot.activeTabId) ?? snapshot.tabs[0], [snapshot]);
    const blank = activeTab?.url === BLANK_URL;
    /* A screen sliding on or off stage is on the window without being the screen
       the session is on, and its page travels with it rather than waiting off
       screen for it to land. Only a painting screen may: one parked off stage
       still measures a rect over the window, and the stage moves for all of them
       at once. */
    const travelling = moving && painted && !!placement && placement.x + placement.width > 0 && placement.x < window.innerWidth;
    const shown = (visible || travelling) && !occluded && !blank && !!activeTab;

    /* The bar follows the page until someone starts typing in it, and goes back
       to following once they are done. Pages move on their own — a click inside
       a web app changes the address — and that must not eat a half-typed one. */
    const pageAddress = blank ? "" : (activeTab?.url ?? "");
    const address = typed ?? pageAddress;
    useEffect(() => setTyped(null), [activeTab?.id]);

    useLayoutEffect(() => {
        const host = viewportRef.current;
        if (!host) return;
        let frame = 0;
        const measure = () => {
            frame = 0;
            const rect = host.getBoundingClientRect();
            const next = {
                x: Math.round(rect.left),
                y: Math.round(rect.top),
                width: Math.max(1, Math.round(rect.width)),
                height: Math.max(1, Math.round(rect.height)),
            };
            setPlacement((previous) => (sameBounds(previous, next) ? previous : next));
        };
        /* Layout settles once per frame; a divider drag fires far more often. */
        const schedule = () => {
            if (!frame) frame = window.requestAnimationFrame(measure);
        };
        measureRef.current = measure;
        measure();
        const observer = new ResizeObserver(schedule);
        observer.observe(host);
        const scrollers = scrollParents(host);
        for (const scroller of scrollers) scroller.addEventListener("scroll", schedule, { passive: true });
        window.addEventListener("resize", schedule);
        window.addEventListener("transitionend", schedule, true);
        return () => {
            observer.disconnect();
            if (frame) window.cancelAnimationFrame(frame);
            for (const scroller of scrollers) scroller.removeEventListener("scroll", schedule);
            window.removeEventListener("resize", schedule);
            window.removeEventListener("transitionend", schedule, true);
        };
    }, []);

    /* Nothing reports the stage sliding the way a scroll or a resize would, so
       the page area is read again on every frame of the travel, and once more
       where it lands. */
    useEffect(() => {
        measureRef.current();
        if (!moving) return;
        return onStageFrame(() => measureRef.current());
    }, [moving]);

    useEffect(() => {
        void browserApi.setBounds(agentId, shown && placement ? placement : null).catch(reportError("place browser page"));
    }, [agentId, placement, shown]);

    useEffect(
        () => () => {
            void browserApi.setBounds(agentId, null).catch(() => {});
        },
        [agentId],
    );

    const run = (operation: Promise<unknown>, label: string) => {
        void operation.then(() => refresh()).catch(reportError(label));
    };

    return (
        <section className={`browser-pane ${agentType}`} data-browser-pane data-agent-id={agentId} aria-label={`${agentType} browser`}>
            <TabBar
                variant="browser"
                ariaLabel="Browser tabs"
                tabs={snapshot.tabs.map((tab) => ({
                    id: tab.id,
                    label: tab.title || (tab.url === BLANK_URL ? "New tab" : tab.url),
                    title: tab.url,
                    active: tab.id === snapshot.activeTabId,
                    className: tab.acting ? "acting" : undefined,
                    icon: <SiteIcon src={tab.favicon} />,
                    badge: tab.acting ? (
                        <span className={`agent-glyph ${agentType}`} role="img" aria-label={`${agentType} is working in this tab`}>
                            <AgentIcon type={agentType} size={16} />
                        </span>
                    ) : undefined,
                    accessory: tab.loading ? (
                        <span className="agent-activity state-working" role="img" aria-label="Loading">
                            <span className="agent-state-loader" aria-hidden="true" />
                        </span>
                    ) : undefined,
                }))}
                onSelect={(id) => run(browserApi.switchTab(agentId, id), "switch browser tab")}
                onClose={(id) => run(browserApi.closeTab(agentId, id), "close browser tab")}
                onAdd={() => run(browserApi.newTab(agentId), "new browser tab")}
                addIcon={<IconPlus size={13} />}
                addTitle="New browser tab — ⌘T"
                addLabel="New browser tab — Command T"
            />
            <form
                className={`browser-toolbar${activeTab?.loading ? " loading" : ""}`}
                onSubmit={(event) => {
                    event.preventDefault();
                    setTyped(null);
                    run(browserApi.navigate(agentId, address), "navigate browser");
                }}>
                <button
                    type="button"
                    aria-label="Back"
                    title="Back — ⌘["
                    disabled={!activeTab?.canGoBack}
                    onClick={() => run(browserApi.back(agentId), "browser back")}>
                    <IconChevron size={13} className="browser-back-icon" />
                </button>
                <button
                    type="button"
                    aria-label="Forward"
                    title="Forward — ⌘]"
                    disabled={!activeTab?.canGoForward}
                    onClick={() => run(browserApi.forward(agentId), "browser forward")}>
                    <IconChevron size={13} />
                </button>
                <button type="button" aria-label="Reload" title="Reload — ⌘R" onClick={() => run(browserApi.reload(agentId), "reload browser")}>
                    <IconRefresh size={13} />
                </button>
                <input
                    className="browser-address"
                    aria-label="Address and search"
                    value={address}
                    placeholder="Search or enter address"
                    spellCheck={false}
                    onFocus={(event) => event.currentTarget.select()}
                    onBlur={() => setTyped(null)}
                    onChange={(event) => setTyped(event.target.value)}
                />
            </form>
            <div ref={viewportRef} className="browser-viewport" tabIndex={-1}>
                {blank && <div className="browser-blank" aria-label="Blank browser page" />}
            </div>
        </section>
    );
}
