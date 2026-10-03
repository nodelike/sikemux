import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, type RefObject } from "react";
import { animate } from "../lib/motion";
import { deskAppearing, onDeskMotion } from "../state/deskMotion";
import { browserApi, BLANK_URL, type BrowserBounds, type BrowserHole, type BrowserSnapshot } from "../api/browser";
import { onStageFrame, stageMoving, useNativeViewHoles, useNativeViewsOccluded, useStageMoving, type NativeViewHole } from "../state/nativeViews";
import type { AgentType, PtyContext, Session, Window as WindowT } from "../state/types";
import { reportError } from "../state/toast";
import { AgentIcon, IconChevron, IconPhone, IconPlus, IconRefresh, WindowIcon } from "../ui/Icons";
import { FileIcon } from "../ui/FileIcon";
import { SiteIcon } from "../ui/SiteIcon";
import { AddressBar } from "./AddressBar";
import { FloatingAddress } from "./FloatingAddress";
import { forgetPageStill, usePageStill, useStillUpkeep } from "./pageStills";
import { TabBar, type TabDescriptor } from "./TabBar";
import { getState, useStore } from "../state/store";
import { refreshBrowserStrip } from "../state/browserStrips";
import {
    BROWSER_ACTIVE,
    deskEditorId,
    deskItems,
    deskItemsOf,
    EMPTY_DESK,
    EMPTY_STRIP,
    isShown,
    shownDeskItem,
    takeDeskRestore,
    simulatorKey,
    terminalKey,
} from "../state/desks";
import { TerminalPane } from "../terminal/TerminalPane";
import { basename } from "../lib/paths";
import * as cmd from "../state/commands";
import { useShortcutLabel, withShortcut } from "../commands/useShortcutLabel";

const EditorPane = lazy(() => import("../editor/EditorPane").then((module) => ({ default: module.EditorPane })));
const SimulatorPane = lazy(() => import("../sim/SimulatorPane").then((module) => ({ default: module.SimulatorPane })));
const NO_FILES: readonly string[] = [];
const NO_DIRTY: readonly string[] = [];
/** How dark the page goes under the ⌘L address, so the panel stands apart from it. */
const UNDER_ADDRESS_DIM = 0.2;

/*
 * Which scrollers can move this pane on screen: its own scrolling ancestors,
 * and the window. Listening on the window in the capture phase instead meant
 * every scroll anywhere in the app — a chat transcript, a terminal, a file tree
 * — asked the page to re-measure itself.
 */
function scrollParents(element: HTMLElement): (HTMLElement | Window)[] {
    const parents: (HTMLElement | Window)[] = [window];
    for (let node = element.parentElement; node; node = node.parentElement) {
        const style = getComputedStyle(node);
        if (/auto|scroll|overlay/.test(`${style.overflowX} ${style.overflowY}`)) parents.push(node);
    }
    return parents;
}

type Placement = Omit<BrowserBounds, "holes">;

/** The holes that fall on the page, moved into its own coordinates. */
function holesOver(placement: Placement, holes: NativeViewHole[]): BrowserHole[] {
    return holes
        .filter(
            (hole) =>
                hole.x < placement.x + placement.width &&
                hole.x + hole.width > placement.x &&
                hole.y < placement.y + placement.height &&
                hole.y + hole.height > placement.y,
        )
        .map((hole) => ({ ...hole, x: hole.x - placement.x, y: hole.y - placement.y }));
}

/**
 * The live page while its picture travels in its place: masked away entirely,
 * and already where its screen will come to rest, so the stage stopping only
 * has to lift the mask. It stays shown the whole time, because a page shown
 * again after being hidden can draw a blank frame before it repaints.
 */
function maskedWhereItLands(host: HTMLElement, placement: Placement): BrowserBounds {
    const layer = host.closest(".window-layer")?.getBoundingClientRect();
    const stage = host.closest(".window-area")?.getBoundingClientRect();
    const travelled = layer && stage ? layer.left - stage.left : 0;
    return {
        ...placement,
        x: Math.round(host.getBoundingClientRect().left - travelled),
        clipLeft: placement.width,
        clipRight: 0,
        holes: [],
    };
}

/** Where the page area sits in the window, less whatever pokes past its pane's or the stage's sides. */
function measurePage(host: HTMLElement): Placement {
    const rect = host.getBoundingClientRect();
    const stage = host.closest(".window-area")?.getBoundingClientRect();
    const pane = host.closest(".pane")?.getBoundingClientRect();
    const x = Math.round(rect.left);
    const width = Math.max(1, Math.round(rect.width));
    const left = Math.round(Math.max(stage?.left ?? -Infinity, pane?.left ?? -Infinity));
    const right = Math.round(Math.min(stage?.right ?? Infinity, pane?.right ?? Infinity));
    const clipLeft = Math.min(width, Math.max(0, left - x));
    return {
        x,
        y: Math.round(rect.top),
        width,
        height: Math.max(1, Math.round(rect.height)),
        clipLeft,
        clipRight: Math.min(width - clipLeft, Math.max(0, x + width - right)),
    };
}

/**
 * An agent's desk, as an ordinary leaf in the window layout: its browser
 * pages, the files it opened and the task terminals it started, under one
 * strip.
 *
 * It is a sibling of the agent it belongs to rather than something drawn
 * inside it, so it is split, resized, focused and closed by the same layout
 * the terminals use. `deskPanes` is what ties it back to its agent.
 */
export function DeskHost({
    paneId,
    session,
    win,
    active,
    visible,
    painted,
    onEmpty,
}: {
    paneId: string;
    session: Session;
    win: WindowT;
    active: boolean;
    visible: boolean;
    painted: boolean;
    onEmpty: () => void;
}) {
    const agentId = useStore((state) => state.deskPanes[paneId]);
    const agentType = useStore((state) => (agentId ? state.agents[agentId]?.type : undefined));
    const cwd = useStore((state) => (agentId ? state.agents[agentId]?.cwd : undefined)) || session.cwd;
    /* Restored from a layout whose agent is gone — the association is the only
       thing that made this pane mean anything, so it closes. */
    const orphaned = !agentId || !agentType;
    useEffect(() => {
        if (orphaned) onEmpty();
    }, [onEmpty, orphaned]);
    if (orphaned) return null;
    return (
        <DeskSession
            key={agentId}
            paneId={paneId}
            agentId={agentId}
            agentType={agentType}
            cwd={cwd}
            session={session}
            win={win}
            active={active}
            visible={visible}
            painted={painted}
            onEmpty={onEmpty}
        />
    );
}

/* While the split opens or closes around it, the desk keeps the layout it has
   when open and its pane's edge moves over it, so nothing inside rewraps or
   grows a scrollbar part way. It fades along, and a fade turned around mid-way
   starts from the opacity it had reached. */
function useDeskMotion(section: RefObject<HTMLElement | null>, paneId: string): void {
    const fade = useRef<Animation | null>(null);
    useLayoutEffect(
        () =>
            onDeskMotion(paneId, (event) => {
                const desk = section.current;
                const pane = desk?.closest<HTMLElement>(".pane") ?? desk;
                if (!desk || !pane) return;
                if (event.kind === "settled") {
                    desk.style.width = "";
                    return;
                }
                if (!desk.style.width) {
                    const area = desk.closest(".window-area")?.getBoundingClientRect().width ?? 0;
                    desk.style.width = `${desk.getBoundingClientRect().width + area * (event.openShare - event.currentShare)}px`;
                }
                const from = event.appearing ? 0 : Number(getComputedStyle(pane).opacity);
                fade.current?.cancel();
                fade.current = animate(pane, [{ opacity: from }, { opacity: event.heading === "open" ? 1 : 0 }], {
                    duration: event.ms,
                    fill: "forwards",
                });
            }),
        [paneId, section],
    );
}

function DeskSession({
    paneId,
    agentId,
    agentType,
    cwd,
    session,
    win,
    active,
    visible,
    painted,
    onEmpty,
}: {
    paneId: string;
    agentId: string;
    agentType: AgentType;
    cwd: string;
    session: Session;
    win: WindowT;
    active: boolean;
    visible: boolean;
    painted: boolean;
    onEmpty: () => void;
}) {
    const newTabShortcut = useShortcutLabel("browser.tabNew");
    const snapshot = useStore((state) => state.browserStrips[agentId]) ?? EMPTY_STRIP;
    const desk = useStore((state) => state.desks[agentId]) ?? EMPTY_DESK;
    const editorId = deskEditorId(agentId);
    const files = useStore((state) => state.editorViews[editorId]?.openTabs) ?? NO_FILES;
    const dirty = useStore((state) => state.dirtyEditorPaths[editorId]) ?? NO_DIRTY;
    const restoring = useStore((state) => !!state.deskRestores[paneId]);
    const items = useMemo(() => deskItems(desk, snapshot, files), [desk, snapshot, files]);
    const shown = shownDeskItem(desk, items);

    const refresh = useCallback(async () => {
        await refreshBrowserStrip(agentId);
    }, [agentId]);

    /* The app keeps the strips up to date for every desk; this one only has to
       ask for the first read, since its agent may have had no browser at all
       until the click that opened this desk. */
    useEffect(() => {
        if (!visible) return;
        void refresh().catch((error) => console.warn("browser session read failed", error));
    }, [refresh, visible]);

    /*
     * Pages saved by the last run wait here until someone looks at the desk, so
     * a restart does not spend a page on every browser that was left open.
     */
    useEffect(() => {
        if (!visible || !restoring) return;
        const saved = takeDeskRestore(paneId);
        if (!saved || saved.tabs.length === 0) return;
        void (async () => {
            const opened: string[] = [];
            for (const tab of saved.tabs) opened.push(await browserApi.newTab(agentId, tab.url));
            const active = opened[saved.activeIndex];
            if (active) await browserApi.switchTab(agentId, active);
            await refresh();
        })().catch((error) => {
            reportError("restore browser tabs")(error);
            if (deskItemsOf(getState(), agentId).length === 0) onEmpty();
        });
    }, [agentId, onEmpty, paneId, refresh, restoring, visible]);

    /*
     * The desk exists because something is on it, so when the last thing goes
     * it has nothing left to show and closes itself.
     *
     * It has to have held something first. The desk is opened by the same click
     * that asks for a page or a file, and that arrives a round trip later —
     * closing on an empty desk alone would shut it before its first tab landed.
     */
    const heldSomething = useRef(false);
    if (items.length > 0) heldSomething.current = true;
    useEffect(() => {
        if (!visible || restoring || !heldSomething.current || items.length > 0 || desk.reveal) return;
        onEmpty();
    }, [desk.reveal, items.length, onEmpty, restoring, visible]);

    const tabs = items.map((item): TabDescriptor => {
        const tabActive = isShown(item, shown, snapshot);
        if (item.kind === "browser") {
            const { tab } = item;
            return {
                id: item.key,
                label: tab.title || (tab.url === BLANK_URL ? "New tab" : tab.url),
                title: tab.url,
                active: tabActive,
                className: tab.acting ? "acting" : undefined,
                icon: <SiteIcon src={tab.favicon} />,
                accessory: tab.acting ? (
                    <span className={`agent-glyph ${agentType}`} role="img" aria-label={`${agentType} is working in this tab`}>
                        <AgentIcon type={agentType} size={16} />
                    </span>
                ) : tab.loading ? (
                    <span className="agent-activity state-working" role="img" aria-label="Loading">
                        <span className="loading-ring" aria-hidden="true" />
                    </span>
                ) : undefined,
            };
        }
        if (item.kind === "file") {
            const name = basename(item.path);
            return {
                id: item.key,
                label: name,
                title: item.path,
                active: tabActive,
                icon: <FileIcon name={name} size={18} />,
                dirty: dirty.includes(item.path),
            };
        }
        if (item.kind === "simulator") {
            const label = item.simulator.deviceName ?? "Simulator";
            return {
                id: item.key,
                label,
                title: `iOS Simulator · ${label}`,
                active: tabActive,
                icon: (
                    <span className="agent-glyph sim">
                        <IconPhone size={13} />
                    </span>
                ),
            };
        }
        return {
            id: item.key,
            label: item.terminal.label,
            title: item.terminal.label,
            active: tabActive,
            icon: (
                <span className="agent-glyph term">
                    <WindowIcon role="term" size={13} />
                </span>
            ),
        };
    });
    const itemFor = (key: string) => items.find((item) => item.key === key);
    const showingFile = !!shown?.startsWith("file:");
    const context = (id: string): PtyContext => ({
        sessionId: session.id,
        sessionName: session.name,
        sessionKind: session.kind,
        ...(session.kind === "project" && session.cwd ? { project: session.cwd } : {}),
        windowId: win.id,
        paneId: id,
        agentId,
        agentType,
    });

    const sectionRef = useRef<HTMLElement>(null);
    useDeskMotion(sectionRef, paneId);

    return (
        <section ref={sectionRef} className={`desk ${agentType}`} data-desk data-agent-id={agentId} aria-label={`${agentType} desk`}>
            <TabBar
                variant="desk"
                ariaLabel="Desk tabs"
                tabs={tabs}
                onSelect={(key) => {
                    const item = itemFor(key);
                    if (item) cmd.selectDeskItem(agentId, item);
                }}
                onClose={(key) => {
                    const item = itemFor(key);
                    if (item) cmd.closeDeskItem(agentId, item);
                }}
                onAdd={() => cmd.newBrowserTab(agentId)}
                addIcon={<IconPlus size={13} />}
                addTitle={withShortcut("New browser tab", newTabShortcut)}
                addLabel="New browser tab"
            />
            <div className="desk-body">
                <BrowserPage
                    paneId={paneId}
                    agentId={agentId}
                    hidden={shown !== BROWSER_ACTIVE}
                    visible={visible}
                    painted={painted}
                    snapshot={snapshot}
                    refresh={refresh}
                />
                {(files.length > 0 || desk.reveal) && (
                    <div className="desk-editor" hidden={!showingFile}>
                        <Suspense fallback={null}>
                            <EditorPane
                                bare
                                paneId={editorId}
                                cwd={cwd}
                                active={active && showingFile}
                                visible={visible && showingFile}
                                reveal={desk.reveal}
                                onRevealed={(seq) => cmd.consumeDeskReveal(agentId, seq)}
                            />
                        </Suspense>
                    </div>
                )}
                {desk.terminals.map((terminal) => {
                    const showing = shown === terminalKey(terminal.id);
                    return (
                        <div key={terminal.id} className="desk-terminal" hidden={!showing}>
                            <TerminalPane
                                cwd={terminal.cwd}
                                active={active && showing}
                                visible={visible && showing}
                                context={context(terminal.id)}
                                externallyOwned
                            />
                        </div>
                    );
                })}
                {desk.simulators.map((simulator) => {
                    const showing = shown === simulatorKey(simulator.id);
                    return (
                        <div key={simulator.id} className="desk-simulator" hidden={!showing}>
                            <Suspense fallback={null}>
                                <SimulatorPane agentId={agentId} simulator={simulator} visible={visible && showing} />
                            </Suspense>
                        </div>
                    );
                })}
            </div>
        </section>
    );
}

/* The page itself is a native view the window draws over this pane, so the
   pane's only job for it is to say where the page area is. */
function BrowserPage({
    paneId,
    agentId,
    hidden,
    visible,
    painted,
    snapshot,
    refresh,
}: {
    paneId: string;
    agentId: string;
    hidden: boolean;
    visible: boolean;
    painted: boolean;
    snapshot: BrowserSnapshot;
    refresh: (signal?: AbortSignal) => Promise<void>;
}) {
    const backShortcut = useShortcutLabel("browser.back");
    const forwardShortcut = useShortcutLabel("browser.forward");
    const reloadShortcut = useShortcutLabel("browser.reload");
    const viewportRef = useRef<HTMLDivElement>(null);
    const placeRef = useRef<() => void>(() => {});
    const occluded = useNativeViewsOccluded();
    const appHoles = useNativeViewHoles();
    const moving = useStageMoving();
    const activeTab = useMemo(() => snapshot.tabs.find((tab) => tab.id === snapshot.activeTabId) ?? snapshot.tabs[0], [snapshot]);
    const blank = activeTab?.url === BLANK_URL;
    const hasTab = !!activeTab;
    const still = usePageStill(agentId, activeTab?.id);
    const hasStill = !!still;
    useStillUpkeep(
        agentId,
        activeTab?.id,
        visible && !hidden && !occluded && !blank && !moving,
        `${activeTab?.url}|${activeTab?.title}|${activeTab?.loading}|${activeTab?.acting}`,
    );

    const pageAddress = blank ? "" : (activeTab?.url ?? "");
    const addressFloating = useStore((state) => state.deskAddressOpen === agentId) && visible && !hidden;

    const inputs = { paneId, agentId, visible, painted, hidden, occluded, blank, hasTab, hasStill, appHoles, addressFloating };
    const inputsRef = useRef(inputs);
    inputsRef.current = inputs;

    /* The page is placed straight from a measurement, never through React state:
       while the stage slides that runs every frame, and a render plus an effect
       in between would leave the page a frame or more behind its pane. */
    useLayoutEffect(() => {
        const host = viewportRef.current;
        if (!host) return;
        let frame = 0;
        let sent = "";
        const place = () => {
            frame = 0;
            const { paneId, visible, painted, hidden, occluded, blank, hasTab, hasStill, appHoles, addressFloating } = inputsRef.current;
            const placement = measurePage(host);
            if (stageMoving() && hasStill && painted && !hidden && !occluded && !blank && hasTab) {
                send(maskedWhereItLands(host, placement));
                return;
            }
            /* A screen sliding on or off stage is on the window without being the
               screen the session is on, and its page travels with it rather than
               waiting off screen for it to land. Only a painting screen may: one
               parked off stage still measures a rect over the window, and the stage
               moves for all of them at once. */
            const travelling = stageMoving() && painted && placement.clipLeft + placement.clipRight < placement.width;
            const shown = (visible || travelling) && !hidden && !occluded && !blank && hasTab;
            /* A native page cannot inherit CSS opacity, so it is told its pane's. */
            const opacity = deskAppearing(paneId) ? 0 : Math.round(Number(getComputedStyle(host.closest(".pane") ?? host).opacity) * 100) / 100;
            const bounds: BrowserBounds | null = shown
                ? {
                      ...placement,
                      holes: holesOver(placement, appHoles),
                      ...(addressFloating ? { dim: UNDER_ADDRESS_DIM } : {}),
                      ...(opacity < 1 ? { opacity } : {}),
                  }
                : null;
            send(bounds);
        };
        const send = (bounds: BrowserBounds | null) => {
            const key = JSON.stringify(bounds);
            if (key === sent) return;
            sent = key;
            void browserApi.setBounds(inputsRef.current.agentId, bounds).catch(reportError("place browser page"));
        };
        /* Layout settles once per frame; a divider drag fires far more often. */
        const schedule = () => {
            if (!frame) frame = window.requestAnimationFrame(place);
        };
        placeRef.current = place;
        place();
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

    useEffect(() => placeRef.current(), [agentId, visible, painted, hidden, occluded, blank, hasTab, hasStill, appHoles, addressFloating]);

    /* Nothing reports the stage sliding the way a scroll or a resize would, so
       the page area is read again on every frame of the travel, and once more
       where it lands. */
    useEffect(() => {
        placeRef.current();
        if (!moving) return;
        return onStageFrame(() => placeRef.current());
    }, [moving]);

    useEffect(
        () => () => {
            void browserApi.setBounds(agentId, null).catch(() => {});
            forgetPageStill(agentId);
        },
        [agentId],
    );

    const run = (operation: Promise<unknown>, label: string) => {
        void operation.then(() => refresh()).catch(reportError(label));
    };
    const go = (url: string) => run(browserApi.navigate(agentId, url), "navigate browser");

    return (
        <div className="desk-page" hidden={hidden} data-browser-pane>
            <div className={`browser-toolbar${activeTab?.loading ? " loading" : ""}`}>
                <button
                    type="button"
                    aria-label="Back"
                    title={withShortcut("Back", backShortcut)}
                    disabled={!activeTab?.canGoBack}
                    onClick={() => run(browserApi.back(agentId), "browser back")}>
                    <IconChevron size={13} className="browser-back-icon" />
                </button>
                <button
                    type="button"
                    aria-label="Forward"
                    title={withShortcut("Forward", forwardShortcut)}
                    disabled={!activeTab?.canGoForward}
                    onClick={() => run(browserApi.forward(agentId), "browser forward")}>
                    <IconChevron size={13} />
                </button>
                <button
                    type="button"
                    aria-label="Reload"
                    title={withShortcut("Reload", reloadShortcut)}
                    onClick={() => run(browserApi.reload(agentId), "reload browser")}>
                    <IconRefresh size={13} />
                </button>
                <AddressBar tabId={activeTab?.id} pageAddress={pageAddress} onGo={go} vacant={addressFloating} />
            </div>
            <div ref={viewportRef} className="browser-viewport" tabIndex={-1}>
                {/* Under the live page, where it shows only while the stage moves. */}
                {still && !blank && (
                    <img
                        className="browser-still"
                        src={still}
                        srcSet={`${still} ${window.devicePixelRatio || 1}x`}
                        alt=""
                        decoding="async"
                        draggable={false}
                    />
                )}
                {blank && <div className="browser-blank" aria-label="Blank browser page" />}
                {blank && addressFloating && <div className="browser-dim" style={{ opacity: UNDER_ADDRESS_DIM }} />}
            </div>
            {addressFloating && (
                <FloatingAddress over={viewportRef} tabId={activeTab?.id} pageAddress={pageAddress} onGo={go} onClose={cmd.closeDeskAddress} />
            )}
        </div>
    );
}
