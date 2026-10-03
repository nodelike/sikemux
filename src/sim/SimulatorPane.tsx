import { useCallback, useEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { simApi, type SimDevice, type SimOrientation, type SimScreen, type SimStatus, type SimStreamFormat } from "../api/sim";
import * as cmd from "../state/commands";
import type { DeskSimulator } from "../state/types";
import { notify, reportError } from "../state/toast";
import { Dropdown } from "../ui/Dropdown";
import { EmptyState } from "../ui/Panel";
import { playScreen } from "./screenStream";
import "../styles/simulator.css";

const NAMED_KEYS = new Set(["Enter", "Escape", "Backspace", "Tab", "Delete", "ArrowRight", "ArrowLeft", "ArrowDown", "ArrowUp"]);
const TURNS: SimOrientation[] = ["portrait", "landscapeLeft", "portraitUpsideDown", "landscapeRight"];

/** Where a pointer is on the device, in points, for a canvas drawn with `object-fit: contain`. */
export function devicePoint(
    canvas: { width: number; height: number; rect: { left: number; top: number; width: number; height: number } },
    screen: SimScreen,
    clientX: number,
    clientY: number,
): { x: number; y: number } | null {
    if (!canvas.width || !canvas.height) return null;
    const fit = Math.min(canvas.rect.width / canvas.width, canvas.rect.height / canvas.height);
    const left = canvas.rect.left + (canvas.rect.width - canvas.width * fit) / 2;
    const top = canvas.rect.top + (canvas.rect.height - canvas.height * fit) / 2;
    const x = ((clientX - left) / fit) * (screen.width / canvas.width);
    const y = ((clientY - top) / fit) * (screen.height / canvas.height);
    return x < 0 || y < 0 || x > screen.width || y > screen.height ? null : { x, y };
}

export function SimulatorPane({ agentId, simulator, visible }: { agentId: string; simulator: DeskSimulator; visible: boolean }) {
    const [status, setStatus] = useState<SimStatus | null>(null);
    const [download, setDownload] = useState<number | null>(null);
    const [devices, setDevices] = useState<SimDevice[] | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const [screen, setScreen] = useState<SimScreen | null>(null);
    const [fps, setFps] = useState(0);
    const [format, setFormat] = useState<SimStreamFormat>("h264");
    const [turn, setTurn] = useState(0);
    const [latency, setLatency] = useState<number | null>(null);
    const player = useRef<ReturnType<typeof playScreen> | null>(null);
    const canvasRef = useRef<HTMLCanvasElement>(null);
    // Input goes out in the order it happened; the helper answers requests in parallel.
    const input = useRef<Promise<unknown>>(Promise.resolve());

    const device = devices?.find((candidate) => candidate.udid === simulator.udid) ?? null;
    const udid = device?.udid ?? null;
    const booted = device?.state === "booted";

    const refresh = useCallback(async () => {
        try {
            const list = await simApi.devices();
            setDevices(list);
            setProblem(null);
            const current = list.find((candidate) => candidate.udid === simulator.udid);
            const pick =
                current ?? list.find((candidate) => candidate.state === "booted") ?? list.find((candidate) => candidate.name.startsWith("iPhone"));
            if (pick && pick.udid !== simulator.udid) cmd.setDeskSimulatorDevice(agentId, simulator.id, { udid: pick.udid, name: pick.name });
        } catch (error) {
            setProblem(error instanceof Error ? error.message : String(error));
        }
    }, [agentId, simulator.id, simulator.udid]);

    useEffect(() => {
        let alive = true;
        void simApi.status().then((next) => alive && setStatus(next), reportError("simulator status"));
        return () => {
            alive = false;
        };
    }, []);

    useEffect(() => {
        if (!status?.supported || status.installed) return;
        const controller = new AbortController();
        void simApi.subscribe((event) => setDownload(event.fraction), controller.signal);
        setDownload(0);
        simApi
            .prepare()
            .then(() => setStatus({ ...status, installed: true }))
            .catch((error) => setProblem(String(error)))
            .finally(() => setDownload(null));
        return () => controller.abort();
    }, [status]);

    useEffect(() => {
        if (status?.supported && status.installed && visible) void refresh();
    }, [status, visible, refresh]);

    useEffect(() => {
        if (!udid || !booted) return setScreen(null);
        let alive = true;
        void simApi.screen(udid).then((next) => alive && setScreen(next), reportError("simulator screen size"));
        return () => {
            alive = false;
        };
    }, [udid, booted, turn]);

    useEffect(() => {
        const canvas = canvasRef.current;
        if (!udid || !booted || !visible || !canvas) return;
        const playing = playScreen(udid, canvas, { onFps: setFps, onFormat: setFormat, onError: setProblem, onLatency: setLatency });
        player.current = playing;
        return () => {
            playing.stop();
            player.current = null;
        };
    }, [udid, booted, visible]);

    const send = (work: () => Promise<unknown>) => {
        input.current = input.current.then(work).catch(reportError("simulator input"));
    };

    const pointAt = (event: PointerEvent<HTMLCanvasElement>) => {
        const canvas = canvasRef.current;
        if (!canvas || !screen) return null;
        return devicePoint(
            { width: canvas.width, height: canvas.height, rect: canvas.getBoundingClientRect() },
            screen,
            event.clientX,
            event.clientY,
        );
    };
    const touch = (phase: "down" | "move" | "up") => (event: PointerEvent<HTMLCanvasElement>) => {
        if (!udid || (phase === "move" && !event.currentTarget.hasPointerCapture(event.pointerId))) return;
        if (phase === "down") {
            event.currentTarget.setPointerCapture(event.pointerId);
            player.current?.markInput();
        }
        const point = pointAt(event);
        if (point) send(() => simApi.touch(udid, phase, point.x, point.y));
    };
    const type = (event: KeyboardEvent<HTMLCanvasElement>) => {
        if (!udid || event.metaKey || event.ctrlKey || event.altKey) return;
        if (NAMED_KEYS.has(event.key)) send(() => simApi.key(udid, event.key));
        else if (event.key.length === 1) send(() => simApi.text(udid, event.key));
        else return;
        event.preventDefault();
    };

    const power = async () => {
        if (!udid) return;
        setBusy(true);
        try {
            await (booted ? simApi.shutdown(udid) : simApi.boot(udid));
            await refresh();
        } catch (error) {
            setProblem(error instanceof Error ? error.message : String(error));
        } finally {
            setBusy(false);
        }
    };
    const rotate = () => {
        if (!udid) return;
        const next = (turn + 1) % TURNS.length;
        send(() => simApi.orientation(udid, TURNS[next]).then(() => setTurn(next)));
    };
    const screenshot = () => {
        if (!udid || !device) return;
        const stamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
        send(() =>
            simApi
                .screenshot(udid, `~/Desktop/Simulator ${device.name} ${stamp}.png`)
                .then(() => notify("success", "Screenshot saved to the Desktop")),
        );
    };

    if (status && !status.supported)
        return <EmptyState title="iOS Simulator" message={status.reason ?? "The iOS Simulator is not available here."} />;
    if (download !== null) return <EmptyState title="Getting the simulator helper" message={`Downloading… ${Math.round(download * 100)}%`} />;
    if (problem && !devices) return <EmptyState title="iOS Simulator" message={problem} tone="error" />;
    if (devices && devices.length === 0)
        return <EmptyState title="No iOS simulators" message="Add an iOS runtime in Xcode › Settings › Components, then reopen this tab." />;

    return (
        <div className="sim-pane">
            <div className="sim-bar">
                <Dropdown
                    label="Device"
                    value={udid ?? ""}
                    options={(devices ?? []).map((candidate) => ({
                        value: candidate.udid,
                        label: candidate.name,
                        detail: `${candidate.runtime}${candidate.state === "booted" ? " · running" : ""}`,
                    }))}
                    onChange={(next) => {
                        const picked = devices?.find((candidate) => candidate.udid === next);
                        if (picked) cmd.setDeskSimulatorDevice(agentId, simulator.id, { udid: picked.udid, name: picked.name });
                    }}
                    disabled={!devices}
                />
                <button type="button" className="sim-chip" onClick={() => void power()} disabled={!udid || busy}>
                    {busy ? (booted ? "Shutting down…" : "Booting…") : booted ? "Shut down" : "Boot"}
                </button>
                <span className="sim-chips">
                    <button type="button" className="sim-chip" disabled={!booted} onClick={() => udid && send(() => simApi.button(udid, "home"))}>
                        Home
                    </button>
                    <button type="button" className="sim-chip" disabled={!booted} onClick={() => udid && send(() => simApi.button(udid, "lock"))}>
                        Lock
                    </button>
                    <button type="button" className="sim-chip" disabled={!booted} onClick={rotate}>
                        Rotate
                    </button>
                    <button type="button" className="sim-chip" disabled={!booted} onClick={screenshot}>
                        Screenshot
                    </button>
                </span>
                {import.meta.env.DEV && booted && visible && (
                    <span className="sim-fps" title="Frames drawn in the last second, the format, and the last tap → frame time (dev builds only)">
                        {fps} fps · {format === "h264" ? "H.264" : "MJPEG"}
                        {latency !== null && ` · tap→frame ${Math.round(latency)} ms`}
                    </span>
                )}
            </div>
            {problem && devices && <div className="sim-problem">{problem}</div>}
            <div className="sim-stage">
                {booted ? (
                    <canvas
                        ref={canvasRef}
                        className="sim-screen"
                        tabIndex={0}
                        aria-label={`${device?.name ?? "Simulator"} screen`}
                        onPointerDown={touch("down")}
                        onPointerMove={touch("move")}
                        onPointerUp={touch("up")}
                        onKeyDown={type}
                    />
                ) : (
                    <EmptyState
                        message={device ? `${device.name} is not running.` : "Pick a device."}
                        action={device ? { label: "Boot", onClick: () => void power() } : undefined}
                    />
                )}
            </div>
        </div>
    );
}
