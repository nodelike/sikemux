import { useEffect, useState } from "react";
import { remoteApi, shortKey, spacedCode, type DeviceAccess, type PairedDevice, type PendingDevice, type RemoteStatus } from "../api/remote";
import { reportError } from "../state/toast";
import { Dropdown } from "../ui/Dropdown";
import { Switch } from "../ui/Controls";
import { IconTrash } from "../ui/Icons";
import { PairingQr } from "./PairingQr";
import { SettingsPage, SettingsRow, SettingsRows, SettingsSection } from "./SettingsLayout";

const ACCESS_OPTIONS = [
    { value: "full", label: "Full control", detail: "Drive terminals and agents" },
    { value: "watch", label: "Watch and approve", detail: "Read sessions and answer permission requests" },
];

const PLATFORM_NAMES: Record<string, string> = { ios: "iOS", android: "Android", macos: "macOS", linux: "Linux", web: "Web" };

function platformName(platform: string): string {
    return PLATFORM_NAMES[platform] ?? platform;
}

export function seenLabel(at: number | null, now: number): string {
    if (at === null) return "never connected";
    const minutes = Math.floor((now - at) / 60_000);
    if (minutes < 1) return "seen just now";
    if (minutes < 60) return `seen ${minutes} min ago`;
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return `seen ${hours} h ago`;
    return `seen ${new Date(at).toLocaleDateString()}`;
}

function countdown(expiresAt: number, now: number): string {
    const seconds = Math.max(0, Math.ceil((expiresAt - now) / 1000));
    return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

function useRemoteStatus(): [RemoteStatus | null, (next: Promise<RemoteStatus>, what: string) => Promise<void>] {
    const [status, setStatus] = useState<RemoteStatus | null>(null);
    useEffect(() => {
        const controller = new AbortController();
        remoteApi
            .subscribe(setStatus, controller.signal)
            .then(() => remoteApi.status())
            .then((current) => {
                if (!controller.signal.aborted) setStatus(current);
            })
            .catch((error: unknown) => {
                if (!controller.signal.aborted) reportError("Remote access")(error);
            });
        return () => controller.abort();
    }, []);
    const apply = async (next: Promise<RemoteStatus>, what: string) => {
        try {
            setStatus(await next);
        } catch (error) {
            reportError(what)(error);
        }
    };
    return [status, apply];
}

function useNow(running: boolean): number {
    const [now, setNow] = useState(() => Date.now());
    useEffect(() => {
        if (!running) return;
        setNow(Date.now());
        const timer = window.setInterval(() => setNow(Date.now()), 1000);
        return () => window.clearInterval(timer);
    }, [running]);
    return now;
}

export function DevicesPage() {
    const [status, apply] = useRemoteStatus();
    const now = useNow(Boolean(status?.pairing));
    const pairing = status?.pairing && status.pairing.expiresAt > now ? status.pairing : null;

    return (
        <SettingsPage>
            <SettingsSection
                title="Remote access"
                meta={status ? (status.enabled ? `${status.connected.length} connected` : "off") : "checking"}
                sub="Lets the phones you pair reach this Mac's terminals and agents. Connections are encrypted end to end and go direct when the network allows.">
                <SettingsRows>
                    <SettingsRow
                        label="Allow paired devices"
                        desc="While this is on, the background process keeps running after you quit and starts again when you log in, so your devices can always reach this Mac."
                        asLabel
                        control={
                            <Switch
                                checked={status?.enabled ?? false}
                                disabled={!status}
                                onChange={(enabled) => void apply(remoteApi.setEnabled(enabled), "Remote access")}
                                label="Allow paired devices"
                            />
                        }
                    />
                    {status?.coreId && (
                        <SettingsRow label="This Mac" desc="Your devices recognise this Mac by its key.">
                            <code className="device-key">{shortKey(status.coreId)}</code>
                        </SettingsRow>
                    )}
                </SettingsRows>
            </SettingsSection>

            <SettingsSection
                title="Pair a device"
                sub="Scan the code with Sikemux on your phone, or choose this Mac from the phones on the same network and type the digits.">
                {!status?.enabled ? (
                    <p className="settings-hint">Turn on remote access to pair a device.</p>
                ) : pairing ? (
                    <div className="pairing-code">
                        <PairingQr link={pairing.link} />
                        <span className="pairing-code-copy">
                            <span className="pairing-code-digits" aria-label={`Pairing code ${pairing.code.split("").join(" ")}`}>
                                {spacedCode(pairing.code)}
                            </span>
                            <span className="pairing-code-note">Expires in {countdown(pairing.expiresAt, now)}. Each code pairs one device.</span>
                        </span>
                    </div>
                ) : (
                    <p className="settings-hint">A code lasts five minutes and is withdrawn after five wrong tries.</p>
                )}
                {status?.pending.map((request) => (
                    <PendingRow
                        key={request.id}
                        request={request}
                        onAnswer={(allow, access) => void apply(remoteApi.answerPairing(request.id, allow, access), "Pairing")}
                    />
                ))}
                <div className="settings-actions start">
                    {pairing ? (
                        <button className="settings-btn" type="button" onClick={() => void apply(remoteApi.closePairing(), "Pairing")}>
                            Cancel code
                        </button>
                    ) : (
                        <button
                            className={`settings-btn${status?.pending.length ? "" : " primary"}`}
                            type="button"
                            disabled={!status?.enabled}
                            onClick={() => void apply(remoteApi.openPairing(), "Pairing")}>
                            Pair a device
                        </button>
                    )}
                </div>
            </SettingsSection>

            <SettingsSection title="Paired devices" meta={status ? `${status.devices.length} paired` : undefined}>
                {!status?.devices.length ? (
                    <div className="settings-empty">No devices yet. A device you pair stays paired until you revoke it here.</div>
                ) : (
                    <SettingsRows>
                        {status.devices.map((device) => (
                            <DeviceRow
                                key={device.id}
                                device={device}
                                connected={status.connected.includes(device.id)}
                                now={now}
                                onAccess={(access) => void apply(remoteApi.setDeviceAccess(device.id, access), "Device access")}
                                onRevoke={() => void apply(remoteApi.revokeDevice(device.id), "Revoke device")}
                            />
                        ))}
                    </SettingsRows>
                )}
            </SettingsSection>
        </SettingsPage>
    );
}

function PendingRow({ request, onAnswer }: { request: PendingDevice; onAnswer: (allow: boolean, access: DeviceAccess) => void }) {
    const [access, setAccess] = useState<DeviceAccess>("full");
    return (
        <div className="pairing-request" role="group" aria-label={`${request.name || "A device"} wants to pair`}>
            <span className="settings-row-copy">
                <span className="settings-row-label">{request.name || "Unnamed device"} wants to pair</span>
                <span className="settings-row-desc">
                    {platformName(request.platform)} · key <code className="device-key">{shortKey(request.deviceId)}</code> · it typed the right code
                </span>
            </span>
            <div className="settings-actions">
                <Dropdown
                    className="settings-dd"
                    label="access for this device"
                    value={access}
                    options={ACCESS_OPTIONS}
                    onChange={(value) => setAccess(value as DeviceAccess)}
                />
                <button className="settings-btn" type="button" onClick={() => onAnswer(false, access)}>
                    Decline
                </button>
                <button className="settings-btn primary" type="button" onClick={() => onAnswer(true, access)}>
                    Allow
                </button>
            </div>
        </div>
    );
}

function DeviceRow({
    device,
    connected,
    now,
    onAccess,
    onRevoke,
}: {
    device: PairedDevice;
    connected: boolean;
    now: number;
    onAccess: (access: DeviceAccess) => void;
    onRevoke: () => void;
}) {
    return (
        <SettingsRow
            label={device.name || "Unnamed device"}
            desc={`${platformName(device.platform)} · ${connected ? "connected now" : seenLabel(device.lastSeen, now)}`}>
            <span className="device-controls">
                <Dropdown
                    className="settings-dd"
                    label={`access for ${device.name}`}
                    value={device.access}
                    options={ACCESS_OPTIONS}
                    onChange={(value) => onAccess(value as DeviceAccess)}
                />
                <button className="settings-btn danger" type="button" onClick={onRevoke} title="Forget this device and end its connections">
                    <IconTrash size={12} /> Revoke
                </button>
            </span>
        </SettingsRow>
    );
}
