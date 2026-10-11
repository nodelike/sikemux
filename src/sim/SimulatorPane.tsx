import {
    useCallback,
    useEffect,
    useRef,
    useState,
    type CSSProperties,
    type KeyboardEvent,
    type PointerEvent,
    type ReactNode,
    type WheelEvent,
} from "react";
import { createPortal } from "react-dom";
import {
    simApi,
    type SimButton,
    type SimChrome,
    type SimChromeButton,
    type SimDevice,
    type SimOrientation,
    type SimScreen,
    type SimStreamFormat,
} from "../api/sim";
import { AGENT_NAMES } from "../agents/agentLaunch";
import { readClipboardText } from "../lib/clipboard";
import { basename } from "../lib/paths";
import * as cmd from "../state/commands";
import { confirmDialog } from "../state/dialog";
import { useSimulatorActing, useSimulatorAttachments } from "../state/simulatorAgents";
import { getState, useStore } from "../state/store";
import type { DeskSimulator } from "../state/types";
import { errMessage as message, notify, reportError } from "../state/toast";
import { Dropdown } from "../ui/Dropdown";
import { IconCamera, IconHome, IconLock, IconPhone, IconPower, IconRotate, IconTablet } from "../ui/Icons";
import { EmptyState } from "../ui/Panel";
import { Tooltip } from "../ui/Tooltip";
import { useDocumentVisible } from "./documentVisible";
import { playScreen, type ScreenPlayer } from "./screenStream";
import { loadSimStatus, prepareSim, simUsable, useSimStatus } from "./simStatus";
import "../styles/simulator.css";

const NAMED_KEYS = new Set(["Enter", "Escape", "Backspace", "Delete", "ArrowRight", "ArrowLeft", "ArrowDown", "ArrowUp"]);
const TURNS: SimOrientation[] = ["portrait", "landscapeRight", "portraitUpsideDown", "landscapeLeft"];
const REFRESH_MS = 5000;
const WHEEL_LIFT_MS = 90;
/** A stream that ran this long before ending comes back on its own; one that ends sooner waits for Reconnect. */
const RECONNECT_AFTER_MS = 3000;

export interface Point {
    x: number;
    y: number;
}

export interface CanvasBox {
    width: number;
    height: number;
    rect: { left: number; top: number; width: number; height: number };
}

export const isIosDevice = (device: SimDevice): boolean => /^(iOS|iPadOS)\b/.test(device.runtime);

const clamp = (value: number, max: number) => Math.min(Math.max(value, 0), max);

/** Where a pointer is on the device in points, and how many points one CSS pixel covers. The canvas
    shows the screen turned the way the device is, so a point on it is the device's own point. */
function screenPoint(canvas: CanvasBox, screen: SimScreen, clientX: number, clientY: number): Point & { scale: number } {
    const fit = Math.min(canvas.rect.width / canvas.width, canvas.rect.height / canvas.height);
    const left = canvas.rect.left + (canvas.rect.width - canvas.width * fit) / 2;
    const top = canvas.rect.top + (canvas.rect.height - canvas.height * fit) / 2;
    const scale = screen.width / (canvas.width * fit);
    return { x: (clientX - left) * scale, y: (clientY - top) * scale, scale };
}

/** Where a pointer is on the device, in points. Off the screen it is null, or the nearest edge when clamped. */
export function devicePoint(canvas: CanvasBox, screen: SimScreen, clientX: number, clientY: number, opts: { clamp?: boolean } = {}): Point | null {
    if (!canvas.width || !canvas.height) return null;
    const { x, y } = screenPoint(canvas, screen, clientX, clientY);
    if (opts.clamp) return { x: clamp(x, screen.width), y: clamp(y, screen.height) };
    return x < 0 || y < 0 || x > screen.width || y > screen.height ? null : { x, y };
}

/** Where a finger scrolling by `dx`, `dy` CSS pixels goes next, and whether it ran into the screen's edge. */
export function scrollStep(at: Point, screen: SimScreen, scale: number, dx: number, dy: number): { next: Point; stuck: boolean } {
    const x = at.x - dx * scale;
    const y = at.y - dy * scale;
    const next = { x: clamp(x, screen.width), y: clamp(y, screen.height) };
    return { next, stuck: next.x !== x || next.y !== y };
}

export function keyForDevice(event: { key: string; metaKey: boolean; ctrlKey: boolean }): { key: string } | { text: string } | null {
    if (event.metaKey || event.ctrlKey) return null;
    if (NAMED_KEYS.has(event.key)) return { key: event.key };
    return [...event.key].length === 1 ? { text: event.key } : null;
}

const GROUPS = ["Running", "iPhone", "iPad"] as const;

function deviceGroup(device: SimDevice): (typeof GROUPS)[number] {
    if (device.state === "booted") return "Running";
    return device.name.startsWith("iPad") ? "iPad" : "iPhone";
}

export function deviceOptions(devices: readonly SimDevice[], heldByProject: (udid: string) => string | undefined) {
    return [...devices]
        .sort((a, b) => GROUPS.indexOf(deviceGroup(a)) - GROUPS.indexOf(deviceGroup(b)))
        .map((device) => {
            const held = heldByProject(device.udid);
            return {
                value: device.udid,
                label: device.name,
                group: deviceGroup(device),
                icon: deviceIcon(device.name),
                meta: device.runtime,
                detail: held ? `In use by ${basename(held)}` : undefined,
            };
        });
}

function deviceIcon(name: string | undefined): ReactNode {
    return name?.startsWith("iPad") ? <IconTablet size={14} /> : <IconPhone size={14} />;
}

const TURN_DEGREES: Partial<Record<SimScreen["orientation"], number>> = { landscapeLeft: -90, landscapeRight: 90, portraitUpsideDown: 180 };

export interface FrameLayout {
    /** CSS pixels per point. */
    scale: number;
    width: number;
    height: number;
    frame: { width: number; height: number; degrees: number };
    screen: { left: number; top: number; width: number; height: number };
}

/** Where the device's frame and its screen go in `room` CSS pixels: as large as fits, turned with the device.
    The frame turns about its centre, and the screen with it. */
export function frameLayout(art: SimChrome, screen: SimScreen, room: { width: number; height: number }): FrameLayout | null {
    const width = art.width + art.padding.left + art.padding.right;
    const height = art.height + art.padding.top + art.padding.bottom;
    const sideways = screen.orientation.startsWith("landscape");
    const turnedWidth = sideways ? height : width;
    const turnedHeight = sideways ? width : height;
    const scale = Math.min(room.width / turnedWidth, room.height / turnedHeight);
    if (!(scale > 0)) return null;
    const degrees = TURN_DEGREES[screen.orientation] ?? 0;
    const radians = (degrees * Math.PI) / 180;
    const offsetX = art.padding.left + art.width / 2 - width / 2;
    const offsetY = art.padding.top + art.height / 2 - height / 2;
    const centreX = turnedWidth / 2 + offsetX * Math.cos(radians) - offsetY * Math.sin(radians);
    const centreY = turnedHeight / 2 + offsetX * Math.sin(radians) + offsetY * Math.cos(radians);
    return {
        scale,
        width: turnedWidth * scale,
        height: turnedHeight * scale,
        frame: { width: width * scale, height: height * scale, degrees },
        screen: {
            left: (centreX - screen.width / 2) * scale,
            top: (centreY - screen.height / 2) * scale,
            width: screen.width * scale,
            height: screen.height * scale,
        },
    };
}

export function screenshotPath(deviceName: string, at: Date): string {
    const name =
        deviceName
            .replace(/[^\p{L}\p{N} ._()-]+/gu, "-")
            .replace(/^[\s.-]+/, "")
            .trim() || "Device";
    const stamp = at.toISOString().replace(/[:.]/g, "-").slice(0, 19);
    return `~/Desktop/Simulator ${name} ${stamp}.png`;
}

interface Gesture {
    pointerId: number;
    rect: DOMRect;
    last: Point;
}

interface Scroll {
    at: Point;
    scale: number;
    timer: number;
}

const canvasBox = (canvas: HTMLCanvasElement, rect: DOMRect): CanvasBox => ({ width: canvas.width, height: canvas.height, rect });

/** Where the pane puts its controls in the desk's own strip, and the tab that opens its device menu. */
export interface SimulatorChrome {
    tools: HTMLElement | null;
    dot: HTMLElement | null;
    menu: HTMLElement | null;
    closeMenu: () => void;
}

export function SimulatorPane({
    agentId,
    simulator,
    visible,
    chrome,
}: {
    agentId: string;
    simulator: DeskSimulator;
    visible: boolean;
    chrome: SimulatorChrome;
}) {
    const status = useSimStatus();
    const [statusProblem, setStatusProblem] = useState<string | null>(null);
    const [prepare, setPrepare] = useState<{ fraction: number } | { error: string } | null>(null);
    const [prepareAttempt, setPrepareAttempt] = useState(0);
    const [devices, setDevices] = useState<SimDevice[] | null>(null);
    const [listProblem, setListProblem] = useState<string | null>(null);
    const [streamProblem, setStreamProblem] = useState<string | null>(null);
    const [actionProblem, setActionProblem] = useState<string | null>(null);
    const [power, setPower] = useState<"booting" | "shuttingDown" | null>(null);
    const [screen, setScreen] = useState<SimScreen | null>(null);
    const [framed, setFramed] = useState(false);
    const [fps, setFps] = useState(0);
    const [format, setFormat] = useState<SimStreamFormat>("h264");
    const [latency, setLatency] = useState<number | null>(null);
    const [screenAsked, setScreenAsked] = useState(0);
    const [streamEnded, setStreamEnded] = useState(false);
    const [streamAttempt, setStreamAttempt] = useState(0);
    const player = useRef<ScreenPlayer | null>(null);
    const canvasRef = useRef<HTMLCanvasElement>(null);
    const showFrame = useStore((state) => state.simulatorFrame);
    const [room, setRoom] = useState<{ width: number; height: number } | null>(null);
    const stageRef = useRef<HTMLDivElement>(null);
    useEffect(() => {
        const stage = stageRef.current;
        if (!stage || typeof ResizeObserver === "undefined") return;
        const sizes = new ResizeObserver(([entry]) => setRoom({ width: entry.contentRect.width, height: entry.contentRect.height }));
        sizes.observe(stage);
        return () => sizes.disconnect();
    }, []);
    // Input goes out in the order it happened; the helper answers requests in parallel.
    const input = useRef<Promise<unknown>>(Promise.resolve());
    const gesture = useRef<Gesture | null>(null);
    const pendingMove = useRef<Point | null>(null);
    const scroll = useRef<Scroll | null>(null);
    const latest = useRef(simulator);
    latest.current = simulator;

    const documentVisible = useDocumentVisible();
    const shown = visible && documentVisible;
    const usable = simUsable(status);
    const installed = !!status?.installed;
    const device = devices?.find((candidate) => candidate.udid === simulator.udid) ?? null;
    const udid = device?.udid ?? null;
    const booted = device?.state === "booted";
    const lastSettled = useRef<{ udid: string; booted: boolean } | null>(null);
    if (device && device.state !== "busy") lastSettled.current = { udid: device.udid, booted };
    const wasBooted = lastSettled.current?.udid === udid && lastSettled.current.booted;
    const transition = power ?? (device?.state === "busy" ? (wasBooted ? "shuttingDown" : "booting") : null);
    const starting = transition === "booting";
    const stopping = transition === "shuttingDown";
    const agentType = useStore((state) => state.agents[agentId]?.type);
    const agentName = agentType ? AGENT_NAMES[agentType] : "The agent";
    const attachments = useSimulatorAttachments();
    const acting = useSimulatorActing(agentId);
    const ownDevice = attachments[agentId] ?? null;
    const heldBy = (target: string) =>
        Object.values(attachments).find((attachment) => attachment.udid === target && attachment.agentId !== agentId) ?? null;

    const refresh = useCallback(async () => {
        try {
            const list = (await simApi.devices()).filter(isIosDevice);
            setDevices(list);
            setListProblem(null);
            const { id, udid: showing, deviceName } = latest.current;
            const current = list.find((candidate) => candidate.udid === showing);
            const pick =
                current ?? list.find((candidate) => candidate.state === "booted") ?? list.find((candidate) => candidate.name.startsWith("iPhone"));
            if (pick && (pick.udid !== showing || pick.name !== deviceName))
                cmd.setDeskSimulatorDevice(agentId, id, { udid: pick.udid, name: pick.name });
        } catch (error) {
            setListProblem(message(error));
        }
    }, [agentId]);

    useEffect(() => {
        if (status || statusProblem) return;
        let alive = true;
        loadSimStatus().then(
            () => alive && setStatusProblem(null),
            (error: unknown) => alive && setStatusProblem(message(error)),
        );
        return () => {
            alive = false;
        };
    }, [status, statusProblem]);

    useEffect(() => {
        if (!usable || installed || !shown) return;
        let alive = true;
        const controller = new AbortController();
        void simApi.subscribe((event) => alive && setPrepare({ fraction: event.fraction }), controller.signal).catch(() => {});
        setPrepare({ fraction: 0 });
        prepareSim().then(
            () => alive && setPrepare(null),
            (error: unknown) => alive && setPrepare({ error: message(error) }),
        );
        return () => {
            alive = false;
            controller.abort();
        };
    }, [usable, installed, shown, prepareAttempt]);

    const ready = usable && installed;
    useEffect(() => {
        if (!ready || !shown) return;
        void refresh();
        const timer = window.setInterval(() => {
            void refresh();
            setScreenAsked((asked) => asked + 1);
        }, REFRESH_MS);
        return () => window.clearInterval(timer);
    }, [ready, shown, refresh]);

    useEffect(() => {
        if (!ready) return;
        const controller = new AbortController();
        void simApi
            .subscribeAttached((attached) => {
                if (attached.agentId === agentId) void refresh();
            }, controller.signal)
            .catch(() => {});
        return () => controller.abort();
    }, [ready, agentId, refresh]);

    useEffect(() => {
        if (!udid || !booted) return setScreen(null);
        let alive = true;
        void simApi.screen(udid).then((next) => alive && setScreen(next), reportError("simulator screen size"));
        return () => {
            alive = false;
        };
    }, [udid, booted, screenAsked]);

    useEffect(() => {
        const canvas = canvasRef.current;
        if (!udid || !booted || !shown || !canvas) return;
        setStreamProblem(null);
        setStreamEnded(false);
        const startedAt = performance.now();
        const playing = playScreen(udid, canvas, {
            onError: setStreamProblem,
            onFirstFrame: () => setFramed(true),
            onEnded: () => {
                void refresh();
                if (performance.now() - startedAt > RECONNECT_AFTER_MS) setStreamAttempt((attempt) => attempt + 1);
                else setStreamEnded(true);
            },
            ...(import.meta.env.DEV ? { onFps: setFps, onFormat: setFormat, onLatency: setLatency } : {}),
        });
        player.current = playing;
        if (latestScreen.current) playing.turn(latestScreen.current.orientation);
        playing.clip(latestMask.current);
        return () => {
            playing.stop();
            player.current = null;
        };
    }, [udid, booted, shown, streamAttempt, refresh]);

    useEffect(() => setFramed(false), [udid, booted]);

    const latestMask = useRef<HTMLImageElement | null>(null);
    const [art, setArt] = useState<SimChrome | null>(null);
    useEffect(() => {
        latestMask.current = null;
        player.current?.clip(null);
        setArt(null);
        if (!udid || !booted) return;
        let alive = true;
        void simApi
            .chrome(udid)
            .then((found) => alive && setArt(found))
            .catch((error) => console.warn("simulator frame", error));
        void simApi
            .mask(udid)
            .then(async (url) => {
                if (!url) return;
                const image = new Image();
                image.src = url;
                await image.decode();
                if (!alive) return;
                latestMask.current = image;
                player.current?.clip(image);
            })
            .catch((error) => console.warn("simulator screen outline", error));
        return () => {
            alive = false;
        };
    }, [udid, booted]);

    const latestScreen = useRef<SimScreen | null>(null);
    useEffect(() => {
        latestScreen.current = screen;
        if (screen) player.current?.turn(screen.orientation);
    }, [screen]);

    useEffect(() => {
        const current = gesture.current;
        if (!acting || !current || !udid) return;
        gesture.current = null;
        input.current = input.current.then(() => simApi.touch(udid, "up", current.last.x, current.last.y)).catch(reportError("simulator input"));
    }, [acting, udid]);

    const send = (work: () => Promise<unknown>) => {
        input.current = input.current.then(work).catch(reportError("simulator input"));
    };
    const action = (work: () => Promise<unknown>) => {
        setActionProblem(null);
        send(work);
    };

    const queueMove = (target: string, point: Point) => {
        const waiting = pendingMove.current !== null;
        pendingMove.current = point;
        if (waiting) return;
        send(() => {
            const next = pendingMove.current;
            pendingMove.current = null;
            return next ? simApi.touch(target, "move", next.x, next.y) : Promise.resolve();
        });
    };

    const pointerDown = (event: PointerEvent<HTMLCanvasElement>) => {
        if (!udid || !screen || !framed || acting || gesture.current || scroll.current || !event.isPrimary || event.button !== 0) return;
        const canvas = event.currentTarget;
        const rect = canvas.getBoundingClientRect();
        const point = devicePoint(canvasBox(canvas, rect), screen, event.clientX, event.clientY);
        if (!point) return;
        canvas.setPointerCapture(event.pointerId);
        gesture.current = { pointerId: event.pointerId, rect, last: point };
        setActionProblem(null);
        player.current?.markInput();
        send(() => simApi.touch(udid, "down", point.x, point.y));
    };
    const pointerMove = (event: PointerEvent<HTMLCanvasElement>) => {
        const current = gesture.current;
        if (!udid || !screen || !current || current.pointerId !== event.pointerId) return;
        const point = devicePoint(canvasBox(event.currentTarget, current.rect), screen, event.clientX, event.clientY, { clamp: true });
        if (!point) return;
        current.last = point;
        queueMove(udid, point);
    };
    const pointerEnd = (event: PointerEvent<HTMLCanvasElement>) => {
        const current = gesture.current;
        if (!udid || !current || current.pointerId !== event.pointerId) return;
        gesture.current = null;
        const lifted =
            event.type === "pointerup" && screen
                ? devicePoint(canvasBox(event.currentTarget, current.rect), screen, event.clientX, event.clientY, { clamp: true })
                : null;
        const point = lifted ?? current.last;
        send(() => simApi.touch(udid, "up", point.x, point.y));
    };

    const liftScroll = (target: string) => {
        const current = scroll.current;
        if (!current) return;
        scroll.current = null;
        window.clearTimeout(current.timer);
        send(() => simApi.touch(target, "up", current.at.x, current.at.y));
    };
    const wheel = (event: WheelEvent<HTMLCanvasElement>) => {
        if (!udid || !screen || !framed || acting || gesture.current) return;
        let current = scroll.current;
        if (!current) {
            const canvas = canvasBox(event.currentTarget, event.currentTarget.getBoundingClientRect());
            const start = devicePoint(canvas, screen, event.clientX, event.clientY);
            if (!start) return;
            const scale = screen.width / canvas.rect.width;
            current = scroll.current = { at: start, scale: Math.max(scale, screen.height / canvas.rect.height), timer: 0 };
            player.current?.markInput();
            send(() => simApi.touch(udid, "down", start.x, start.y));
        }
        const lines = event.deltaMode === 1 ? 16 : 1;
        const { next, stuck } = scrollStep(current.at, screen, current.scale, event.deltaX * lines, event.deltaY * lines);
        current.at = next;
        queueMove(udid, next);
        window.clearTimeout(current.timer);
        if (stuck) return liftScroll(udid);
        current.timer = window.setTimeout(() => liftScroll(udid), WHEEL_LIFT_MS);
    };
    useEffect(() => () => window.clearTimeout(scroll.current?.timer), []);

    const keyDown = (event: KeyboardEvent<HTMLCanvasElement>) => {
        if (!udid || acting) return;
        if (event.metaKey && !event.ctrlKey && !event.altKey && event.key.toLowerCase() === "v") {
            event.preventDefault();
            action(() => readClipboardText().then((text) => (text ? simApi.text(udid, text) : undefined)));
            return;
        }
        const press = keyForDevice(event);
        if (!press) return;
        event.preventDefault();
        action(() => ("key" in press ? simApi.key(udid, press.key) : simApi.text(udid, press.text)));
    };

    const togglePower = async () => {
        if (!udid || transition) return;
        setActionProblem(null);
        const holder = Object.values(attachments).find((attachment) => attachment.udid === udid);
        if (booted && holder) {
            const type = getState().agents[holder.agentId]?.type;
            const confirmed = await confirmDialog({
                title: `Shut down ${device?.name ?? "this device"}?`,
                body: `${type ? AGENT_NAMES[type] : "An agent"} is using this device.`,
                confirmLabel: "Shut down",
                destructive: true,
            });
            if (!confirmed) return;
        }
        setPower(booted ? "shuttingDown" : "booting");
        try {
            await (booted ? simApi.shutdown(udid) : simApi.boot(udid));
        } catch (error) {
            setActionProblem(message(error));
        } finally {
            setPower(null);
            await refresh();
        }
    };
    const rotate = () => {
        if (!udid) return;
        const turn = Math.max(
            0,
            TURNS.findIndex((orientation) => orientation === screen?.orientation),
        );
        const next = TURNS[(turn + 1) % TURNS.length];
        action(() => simApi.orientation(udid, next).then(() => setScreenAsked((asked) => asked + 1)));
    };
    const screenshot = () => {
        if (!udid || !device) return;
        const path = screenshotPath(device.name, new Date());
        action(() => simApi.screenshot(udid, path).then(() => notify("success", "Screenshot saved to the Desktop")));
    };

    const chromeParts = (
        <>
            {chrome.dot && booted && createPortal(<span className="sim-live" aria-label="Running" />, chrome.dot)}
            {chrome.tools &&
                createPortal(
                    <>
                        <SimTool label="Home" disabled={!booted} onClick={() => udid && action(() => simApi.button(udid, "home"))}>
                            <IconHome size={14} />
                        </SimTool>
                        <SimTool label="Lock" disabled={!booted} onClick={() => udid && action(() => simApi.button(udid, "lock"))}>
                            <IconLock size={14} />
                        </SimTool>
                        <SimTool label="Rotate" disabled={!booted} onClick={rotate}>
                            <IconRotate size={14} />
                        </SimTool>
                        <SimTool label="Screenshot to the Desktop" disabled={!booted} onClick={screenshot}>
                            <IconCamera size={14} />
                        </SimTool>
                        <span className="sim-tools-gap" />
                        <SimTool
                            label={starting ? "Booting…" : stopping ? "Shutting down…" : booted ? "Shut down" : "Boot"}
                            disabled={!udid || !!transition}
                            onClick={() => void togglePower()}>
                            <IconPower size={14} />
                        </SimTool>
                    </>,
                    chrome.tools,
                )}
            <Dropdown
                label="Device"
                anchor={chrome.menu}
                open={!!chrome.menu}
                onOpenChange={(open) => {
                    if (!open) chrome.closeMenu();
                }}
                menuWidth={280}
                value={udid ?? ""}
                options={deviceOptions(devices ?? [], (candidate) => heldBy(candidate)?.project)}
                onChange={(next) => {
                    const picked = devices?.find((candidate) => candidate.udid === next);
                    if (!picked) return;
                    cmd.setDeskSimulatorDevice(agentId, simulator.id, { udid: picked.udid, name: picked.name });
                    void simApi.setDeskDevice(agentId, picked.udid).catch(reportError("move the agent to that device"));
                }}
            />
        </>
    );

    if (statusProblem)
        return (
            <EmptyState
                title="iOS Simulator"
                message={statusProblem}
                tone="error"
                action={{ label: "Retry", onClick: () => setStatusProblem(null) }}
            />
        );
    if (status && !usable) return <EmptyState title="iOS Simulator" message={status.reason ?? "The iOS Simulator is not available here."} />;
    if (status && !installed) {
        if (prepare && "error" in prepare)
            return (
                <EmptyState
                    title="Could not get the simulator helper"
                    message={prepare.error}
                    tone="error"
                    action={{ label: "Retry", onClick: () => setPrepareAttempt((attempt) => attempt + 1) }}
                />
            );
        return <EmptyState title="Getting the simulator helper" message={`Downloading… ${Math.round((prepare?.fraction ?? 0) * 100)}%`} />;
    }
    if (listProblem && !devices)
        return <EmptyState title="iOS Simulator" message={listProblem} tone="error" action={{ label: "Retry", onClick: () => void refresh() }} />;
    if (devices && devices.length === 0)
        return <EmptyState title="No iOS simulators" message="Add an iOS runtime in Xcode › Settings › Components, then reopen this tab." />;

    const problem = actionProblem ?? streamProblem;
    const layout = showFrame && art && screen && room ? frameLayout(art, screen, room) : null;
    return (
        <div className="sim-pane">
            {chromeParts}
            {ownDevice && ownDevice.udid !== simulator.udid && (
                <div className="sim-note">
                    <span>
                        {agentName} is on {ownDevice.name}
                    </span>
                    <button
                        type="button"
                        className="sim-chip"
                        onClick={() => cmd.setDeskSimulatorDevice(agentId, simulator.id, { udid: ownDevice.udid, name: ownDevice.name })}>
                        Show it
                    </button>
                </div>
            )}
            {problem && <div className="sim-problem">{problem}</div>}
            <div className="sim-stage" ref={stageRef}>
                {booted ? (
                    <>
                        <div
                            className={`sim-device${layout ? " framed" : ""}`}
                            style={layout ? { width: layout.width, height: layout.height } : undefined}>
                            {layout && art && (
                                <div
                                    className="sim-frame"
                                    style={{
                                        width: layout.frame.width,
                                        height: layout.frame.height,
                                        transform: `translate(-50%, -50%) rotate(${layout.frame.degrees}deg)`,
                                    }}>
                                    {art.buttons.map((button) => (
                                        <SideButton
                                            key={button.name}
                                            button={button}
                                            art={art}
                                            scale={layout.scale}
                                            disabled={acting || !udid}
                                            onPress={(phase) => {
                                                const name = SIDE_BUTTONS[button.name];
                                                if (udid && name) send(() => simApi.button(udid, name, phase));
                                            }}
                                        />
                                    ))}
                                    <img
                                        className="sim-bezel"
                                        src={art.image}
                                        alt=""
                                        draggable={false}
                                        style={{
                                            left: art.padding.left * layout.scale,
                                            top: art.padding.top * layout.scale,
                                            width: art.width * layout.scale,
                                            height: art.height * layout.scale,
                                        }}
                                    />
                                </div>
                            )}
                            <canvas
                                ref={canvasRef}
                                style={layout?.screen}
                                className={`sim-screen${framed ? " framed" : ""}${acting ? " locked" : ""}`}
                                tabIndex={0}
                                data-takes-keys
                                aria-label={`${device?.name ?? "Simulator"} screen`}
                                onPointerDown={pointerDown}
                                onPointerMove={pointerMove}
                                onPointerUp={pointerEnd}
                                onPointerCancel={pointerEnd}
                                onLostPointerCapture={pointerEnd}
                                onWheel={wheel}
                                onKeyDown={keyDown}
                            />
                        </div>
                        {import.meta.env.DEV && shown && (
                            <span
                                className="sim-fps"
                                title="Frames drawn in the last second, the format, and the last tap → frame time (dev builds only)">
                                {fps ? `${fps} fps` : "still"} · {format === "h264" ? "H.264" : "MJPEG"}
                                {latency !== null && ` · ${Math.round(latency)} ms`}
                            </span>
                        )}
                        {streamEnded ? (
                            <div className="sim-overlay">
                                <span>The screen stopped.</span>
                                <button type="button" className="sim-chip" onClick={() => setStreamAttempt((attempt) => attempt + 1)}>
                                    Reconnect
                                </button>
                            </div>
                        ) : (
                            acting && (
                                <div className="sim-overlay" role="status">
                                    {agentName} is using the device
                                </div>
                            )
                        )}
                    </>
                ) : transition ? (
                    <EmptyState
                        icon={<span className="loading-ring" />}
                        message={`${starting ? "Booting" : "Shutting down"} ${device?.name ?? "the device"}…`}
                    />
                ) : (
                    <EmptyState
                        message={device ? `${device.name} is not running.` : "Pick a device."}
                        action={device ? { label: "Boot", onClick: () => void togglePower() } : undefined}
                    />
                )}
            </div>
        </div>
    );
}

function SimTool({ label, disabled, onClick, children }: { label: string; disabled: boolean; onClick: () => void; children: ReactNode }) {
    return (
        <Tooltip label={label}>
            <button type="button" className="sim-tool" aria-label={label} disabled={disabled} onClick={onClick}>
                {children}
            </button>
        </Tooltip>
    );
}

const SIDE_BUTTONS: Record<string, SimButton | undefined> = {
    action: "action",
    "volume-up": "volumeUp",
    "volume-down": "volumeDown",
    power: "lock",
};

/** A button on the frame's side: mostly hidden behind the bezel, sliding out under the pointer, pressed while held. */
function SideButton({
    button,
    art,
    scale,
    disabled,
    onPress,
}: {
    button: SimChromeButton;
    art: SimChrome;
    scale: number;
    disabled: boolean;
    onPress: (phase: "down" | "up") => void;
}) {
    const [held, setHeld] = useState(false);
    const width = art.width + art.padding.left + art.padding.right;
    const left = button.anchor === "left";
    const side = left ? art.padding.left : art.padding.right;
    const from = left ? 0 : width - side;
    const at = (x: number) => ((left ? x : width + x - button.width) - from) * scale;
    const release = () => {
        if (!held) return;
        setHeld(false);
        onPress("up");
    };
    return (
        <span
            role="button"
            tabIndex={-1}
            aria-label={button.title}
            aria-disabled={disabled}
            className={`sim-side${held ? " held" : ""}`}
            style={
                {
                    left: from * scale,
                    top: (art.padding.top + button.y) * scale,
                    width: side * scale,
                    height: button.height * scale,
                    "--rest": `${at(button.x)}px`,
                    "--out": `${at(button.hoverX)}px`,
                } as CSSProperties
            }
            onPointerDown={(event) => {
                if (disabled || event.button !== 0) return;
                event.currentTarget.setPointerCapture(event.pointerId);
                setHeld(true);
                onPress("down");
            }}
            onPointerUp={release}
            onPointerCancel={release}
            onLostPointerCapture={release}>
            <img
                src={held && button.imageDown ? button.imageDown : button.image}
                alt=""
                draggable={false}
                style={{ width: button.width * scale, height: button.height * scale }}
            />
        </span>
    );
}
