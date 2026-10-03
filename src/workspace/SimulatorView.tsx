import { useEffect, useMemo, useRef, useState } from "react";
import {
    chromeUrl,
    frameUrl,
    simulatorApi,
    type SimulatorChrome,
    type SimulatorDevice,
    type Orientation,
    type SimulatorInput,
    type SimulatorScreen,
} from "../api/simulator";
import * as cmd from "../state/commands";
import { simulatorKey } from "../state/desks";
import type { DeskSimulator } from "../state/types";
import { notify, reportError } from "../state/toast";
import { Dropdown, type DropdownOption } from "../ui/Dropdown";
import { IconCamera, IconHome, IconLock, IconPhone, IconPower, IconRotate } from "../ui/Icons";
import { shownDeskPaneId } from "../state/selectors";
import { useStore } from "../state/store";
import { useSimulatorsAvailable } from "../state/simulatorAvailable";

interface Box {
    left: number;
    top: number;
    width: number;
    height: number;
}

/**
 * Where a point in the window lands on the device, in points, or null when it
 * misses the screen. The screen is drawn at its own aspect ratio, centred in
 * `stage`, so any margins around it are not part of it. With `clamp`, a point
 * off the screen lands on its nearest edge instead, as a finger dragged past
 * the side of a phone stays on the glass.
 */
export function toDevicePoint(stage: Box, screen: SimulatorScreen, clientX: number, clientY: number, clamp = false): { x: number; y: number } | null {
    const scale = Math.min(stage.width / screen.width, stage.height / screen.height);
    if (!(scale > 0)) return null;
    const left = stage.left + (stage.width - screen.width * scale) / 2;
    const top = stage.top + (stage.height - screen.height * scale) / 2;
    let x = (clientX - left) / scale;
    let y = (clientY - top) / scale;
    const outside = x < 0 || y < 0 || x > screen.width || y > screen.height;
    if (outside && !clamp) return null;
    x = Math.min(Math.max(x, 0), screen.width);
    y = Math.min(Math.max(y, 0), screen.height);
    return { x: Math.round(x * 10) / 10, y: Math.round(y * 10) / 10 };
}

/** The text a key types on the simulator, or null for keys it leaves to Sikemux. */
export function typedText(event: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey">): string | null {
    if (event.metaKey || event.ctrlKey || event.altKey) return null;
    if (event.key === "Enter") return "\n";
    if (event.key === "Tab") return "\t";
    if (event.key === "Backspace") return "\b";
    return [...event.key].length === 1 ? event.key : null;
}

/** Named as Simulator.app names its screenshots: `Simulator Screenshot - iPhone 18 Pro - 2026-10-03 at 10.30.12.png`. */
export function screenshotName(device: string, at: Date): string {
    const two = (value: number) => String(value).padStart(2, "0");
    const day = `${at.getFullYear()}-${two(at.getMonth() + 1)}-${two(at.getDate())}`;
    const time = `${two(at.getHours())}.${two(at.getMinutes())}.${two(at.getSeconds())}`;
    return `Simulator Screenshot - ${device.replaceAll("/", "-")} - ${day} at ${time}.png`;
}

/** The order the rotate button turns through. */
const TURNS: Orientation[] = ["portrait", "landscapeLeft", "landscapeRight"];

/**
 * How far to turn the picture of the screen so it reads upright. The simulator draws a
 * turned app sideways into its upright screen, clockwise for landscapeLeft, so the
 * picture turns back the other way; upright, a point on it is a point in the app.
 */
export function uprightTurn(orientation: Orientation): number {
    switch (orientation) {
        case "landscapeLeft":
            return -90;
        case "landscapeRight":
            return 90;
        case "portraitUpsideDown":
            return 180;
        default:
            return 0;
    }
}

const sideways = (orientation: Orientation) => orientation === "landscapeLeft" || orientation === "landscapeRight";

/** The largest size `content` can be drawn at inside `room`, keeping its proportions. */
export function fitInside(room: { width: number; height: number }, content: { width: number; height: number }) {
    const scale = Math.max(0, Math.min(room.width / content.width, room.height / content.height));
    return { width: content.width * scale, height: content.height * scale };
}

const percent = (part: number, whole: number) => `${(part / whole) * 100}%`;

const osVersion = (os: string) => (os.split(" ").pop() ?? "").split(".").map(Number);

/** Devices to pick from: the newest iOS first, then by name, with the ones already running marked. */
export function devicePickerOptions(devices: readonly SimulatorDevice[]): DropdownOption[] {
    return [...devices]
        .sort((a, b) => {
            const [newer, older] = [osVersion(a.os), osVersion(b.os)];
            for (let at = 0; at < Math.max(newer.length, older.length); at++) {
                const difference = (older[at] ?? 0) - (newer[at] ?? 0);
                if (difference) return difference;
            }
            return a.name.localeCompare(b.name, undefined, { numeric: true });
        })
        .map((device) => ({ value: device.udid, label: device.name, detail: device.booted ? `${device.os} · running` : device.os }));
}

export function SimulatorView({ agentId, simulator, hidden, live }: { agentId: string; simulator: DeskSimulator; hidden?: boolean; live: boolean }) {
    const { udid, screen } = simulator;
    const [devices, setDevices] = useState<SimulatorDevice[]>([]);
    const [booting, setBooting] = useState<string | null>(null);
    const [chrome, setChrome] = useState<SimulatorChrome | null>(null);
    const [orientation, setOrientation] = useState<Orientation>("portrait");
    const [room, setRoom] = useState({ width: 0, height: 0 });
    const stage = useRef<HTMLDivElement>(null);
    const [frame, setFrame] = useState<number | null>(null);
    const [failure, setFailure] = useState<string | null>(null);
    const image = useRef<HTMLImageElement>(null);
    const steps = useRef<Promise<void>>(Promise.resolve());
    const touching = useRef(false);
    const pendingMove = useRef<{ x: number; y: number } | null>(null);

    /* The stream runs only while this tab is on screen, so a desk in the
       background costs the simulator nothing. */
    useEffect(() => {
        if (!live) return;
        const controller = new AbortController();
        setFailure(null);
        void simulatorApi
            .subscribeRotated((rotated) => rotated.udid === udid && setOrientation(rotated.orientation), controller.signal)
            .catch(() => {});
        void simulatorApi
            .subscribeFrames((event) => {
                if (event.udid !== udid) return;
                if (event.error) setFailure(event.error);
                else if (event.frame !== undefined) setFrame(event.frame);
            }, controller.signal)
            .catch(() => {});
        /* Opening the view boots the device if it is off, and makes it the
           agent's, so a tab the person opened works the same as one the agent did. */
        setBooting(simulator.name);
        void simulatorApi
            .attach(agentId, udid)
            .then(() => {
                if (controller.signal.aborted) return;
                setBooting(null);
                void simulatorApi
                    .orientation(udid)
                    .then((turned) => !controller.signal.aborted && setOrientation(turned))
                    .catch(() => {});
                return simulatorApi.openView(udid).then((drawn) => setChrome(drawn ?? null));
            })
            .catch((error) => {
                setBooting(null);
                setFailure(String(error));
            });
        return () => {
            controller.abort();
            void simulatorApi.closeView(udid).catch(() => {});
        };
    }, [agentId, live, simulator.name, udid]);

    useEffect(() => {
        if (!live) return;
        let current = true;
        void simulatorApi
            .devices()
            .then((found) => current && setDevices(found))
            .catch(() => {});
        return () => {
            current = false;
        };
    }, [live]);

    const options = useMemo(
        () => devicePickerOptions(devices.some((device) => device.udid === udid) ? devices : [...devices, { ...simulator, booted: true }]),
        [devices, simulator, udid],
    );

    useEffect(() => {
        const element = stage.current;
        if (!element || typeof ResizeObserver === "undefined") return;
        const observer = new ResizeObserver(([entry]) => setRoom({ width: entry.contentRect.width, height: entry.contentRect.height }));
        observer.observe(element);
        return () => observer.disconnect();
    }, []);

    /* The agent moves to the picked device too, so the person and the agent
       keep looking at the same screen. */
    const pick = (next: string) => {
        if (next === udid) return;
        setBooting(devices.find((device) => device.udid === next)?.name ?? "the simulator");
        void simulatorApi
            .attach(agentId, next)
            .then(({ udid: picked, name, os, screen: size }) => cmd.switchDeskSimulator(agentId, udid, { udid: picked, name, os, screen: size }))
            .catch((error) => {
                setBooting(null);
                reportError("switch the simulator")(error);
            });
    };

    const shutDown = () =>
        void simulatorApi
            .shutdown(udid)
            .then(() => cmd.closeDeskItem(agentId, { key: simulatorKey(udid), kind: "simulator", simulator }))
            .catch(reportError("shut down the simulator"));

    const saveScreenshot = () =>
        void simulatorApi
            .saveScreenshot(udid, screenshotName(simulator.name, new Date()))
            .then((path) => notify("success", `Saved ${path.split("/").pop()} to the Desktop`))
            .catch(reportError("save a screenshot"));

    /* Each step waits for the one before, so the device sees a finger go down,
       move and lift in the order it did. */
    const inOrder = (next: () => Promise<void> | undefined) => {
        steps.current = steps.current.then(next).catch(reportError("control the simulator"));
    };
    const send = (input: SimulatorInput) => inOrder(() => simulatorApi.input(udid, input));

    const pointAt = (clientX: number, clientY: number, clamp = false) => {
        const box = image.current?.getBoundingClientRect();
        if (!box || !screen) return null;
        const shown = sideways(orientation) ? { width: screen.height, height: screen.width } : screen;
        return toDevicePoint(box, shown, clientX, clientY, clamp);
    };

    const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
        const point = pointAt(event.clientX, event.clientY);
        if (!point || event.button !== 0) return;
        event.currentTarget.setPointerCapture(event.pointerId);
        event.currentTarget.classList.add("pointer-focus");
        event.currentTarget.focus();
        touching.current = true;
        send({ type: "touch", phase: "down", ...point });
    };

    /* Moves that arrive while an earlier step is still on its way fold into
       the latest one, so the device follows the finger without a backlog. */
    const onPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
        if (!touching.current) return;
        const point = pointAt(event.clientX, event.clientY, true);
        if (!point) return;
        const queued = pendingMove.current !== null;
        pendingMove.current = point;
        if (queued) return;
        inOrder(() => {
            const latest = pendingMove.current;
            pendingMove.current = null;
            return latest ? simulatorApi.input(udid, { type: "touch", phase: "move", ...latest }) : undefined;
        });
    };

    const lift = (event: React.PointerEvent<HTMLDivElement>) => {
        if (!touching.current) return;
        touching.current = false;
        const point = pointAt(event.clientX, event.clientY, true);
        if (point) send({ type: "touch", phase: "up", ...point });
    };

    const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
        const text = typedText(event);
        if (text === null) return;
        event.preventDefault();
        send({ type: "type", text });
    };

    const label = `${simulator.name} (${simulator.os})`;
    return (
        <div className="desk-simulator" hidden={hidden}>
            <div className="simulator-toolbar">
                <Dropdown
                    className="simulator-picker"
                    value={udid}
                    options={options}
                    onChange={pick}
                    title="Simulator"
                    label={`Simulator: ${label}`}
                    trailing={simulator.os}
                    search="Find a device"
                    menuWidth={280}
                />
            </div>
            <div
                ref={stage}
                className="simulator-stage"
                tabIndex={0}
                role="application"
                aria-label={`${label} screen. Click to tap, drag to swipe, type to enter text.`}
                onPointerDown={onPointerDown}
                onPointerMove={onPointerMove}
                onPointerUp={lift}
                onPointerCancel={lift}
                onKeyDown={onKeyDown}
                onBlur={(event) => event.currentTarget.classList.remove("pointer-focus")}>
                {booting ? (
                    <p className="simulator-status">Starting {booting}…</p>
                ) : failure ? (
                    <p className="simulator-status">The simulator view stopped: {failure}</p>
                ) : frame === null ? (
                    <p className="simulator-status">Connecting to {simulator.name}…</p>
                ) : (
                    <Phone
                        udid={udid}
                        frame={frame}
                        label={label}
                        chrome={chrome}
                        screen={screen}
                        room={room}
                        image={image}
                        turn={uprightTurn(orientation)}
                    />
                )}
            </div>
            <div className="simulator-controls">
                <button type="button" aria-label="Home" title="Home" onClick={() => send({ type: "button", button: "home" })}>
                    <IconHome size={20} />
                </button>
                <button type="button" aria-label="Screenshot" title="Save a screenshot to the Desktop" onClick={saveScreenshot}>
                    <IconCamera size={20} />
                </button>
                <button
                    type="button"
                    aria-label="Rotate"
                    title="Rotate"
                    onClick={() => {
                        const next = TURNS[(TURNS.indexOf(orientation) + 1) % TURNS.length];
                        void simulatorApi.rotate(udid, next).catch(reportError("rotate the simulator"));
                    }}>
                    <IconRotate size={20} />
                </button>
                <button type="button" aria-label="Lock" title="Lock" onClick={() => send({ type: "button", button: "lock" })}>
                    <IconLock size={20} />
                </button>
                <button type="button" aria-label="Shut down" title="Shut down" onClick={shutDown}>
                    <IconPower size={20} />
                </button>
            </div>
        </div>
    );
}

/**
 * The live screen inside the device Xcode draws around it, scaled to fit the
 * stage. Without that drawing the screen shows bare.
 */
function Phone({
    udid,
    frame,
    label,
    chrome,
    screen,
    room,
    image,
    turn,
}: {
    udid: string;
    frame: number;
    label: string;
    chrome: SimulatorChrome | null;
    screen: SimulatorScreen | null;
    room: { width: number; height: number };
    image: React.RefObject<HTMLImageElement | null>;
    turn: number;
}) {
    const whole = chrome ?? screen ?? { width: 1, height: 1 };
    const quarter = Math.abs(turn) === 90;
    const shown = fitInside(room, quarter ? { width: whole.height, height: whole.width } : whole);
    const size = quarter ? { width: shown.height, height: shown.width } : shown;
    const area = chrome?.screen ?? { x: 0, y: 0, ...whole };
    const mask = chrome ? `url("${chromeUrl(udid, "mask")}")` : undefined;
    return (
        <div
            className="simulator-phone"
            style={{
                position: "absolute",
                left: "50%",
                top: "50%",
                width: size.width,
                height: size.height,
                transform: `translate(-50%, -50%) rotate(${turn}deg)`,
            }}>
            {chrome && <img className="simulator-bezel" src={chromeUrl(udid, "chrome")} alt="" draggable={false} />}
            <img
                ref={image}
                className="simulator-screen"
                src={frameUrl(udid, frame)}
                alt={`${label} screen`}
                draggable={false}
                style={{
                    left: percent(area.x, whole.width),
                    top: percent(area.y, whole.height),
                    width: percent(area.width, whole.width),
                    height: percent(area.height, whole.height),
                    maskImage: mask,
                    WebkitMaskImage: mask,
                    maskSize: "100% 100%",
                    WebkitMaskSize: "100% 100%",
                }}
            />
        </div>
    );
}

/** Opens the agent's desk on an iOS simulator, the one the agent uses if it has one. */
export function SimulatorButton({ agentId }: { agentId: string }) {
    const capable = useSimulatorsAvailable();
    const switchedOn = useStore((state) => state.iosSimulator);
    const available = capable && switchedOn;
    const showing = useStore((state) => shownDeskPaneId(state, agentId) !== null && !!state.desks[agentId]?.active?.startsWith("simulator:"));
    if (!available) return null;
    return (
        <button
            type="button"
            className="agent-desk-open"
            aria-pressed={showing}
            aria-label="iOS Simulator"
            title="iOS Simulator"
            onClick={() =>
                void simulatorApi
                    .preferred(agentId)
                    .then(({ udid, name, os, screen }) => cmd.openDeskSimulator(agentId, { udid, name, os, screen }))
                    .catch(reportError("open the iOS Simulator"))
            }>
            <IconPhone size={13} />
        </button>
    );
}
