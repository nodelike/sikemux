import { useEffect, useState, type ReactNode } from "react";
import { accountApi, type AccountStatus } from "../api/account";
import { portsApi } from "../api/ports";
import { AccountAvatar } from "../account/AccountAvatar";
import { loadAccount, offerAccountPhones, setAccount, useAccount } from "../account/account";
import { remoteApi, type AccountLink, type DeviceAccess, type NotificationState, type PairedDevice, type RemoteStatus } from "../api/remote";
import { ACCESS_OPTIONS, platformName } from "../remote/access";
import * as cmd from "../state/commands";
import { reportError, swallow } from "../state/toast";
import { Dropdown } from "../ui/Dropdown";
import { Switch } from "../ui/Controls";
import { IconGlobe, IconTrash } from "../ui/Icons";
import { QrCode } from "./QrCode";
import { SettingsPage, SettingsRow, SettingsRows } from "./SettingsLayout";

const DELETE_ACCOUNT_URL = import.meta.env.DEV ? "http://localhost:5173/delete-account" : "https://app.sikemux.com/delete-account";
export const PHONE_URL = "https://sikemux.com/phone";

/** How a phone's notifications from this host read beside its name, or nothing when it asked for none. */
function notificationNote(state: NotificationState | undefined): string | null {
    switch (state) {
        case "on":
            return "notifications on";
        case "off":
            return "notifications off";
        case "phoneOff":
            return "notifications turned off on the phone";
        case "notReaching":
            return "notifications aren't reaching it";
        case "otherAccount":
            return "notifications need it signed in to your account";
        case "signedOut":
            return "sign in to send it notifications";
        default:
            return null;
    }
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

/** How the live connection to the account reads under the account's email. */
function accountLine(link: AccountLink | null): string {
    if (link?.state === "connecting") return "Connecting to your account";
    if (link?.state === "offline") return "Can't reach your account, retrying";
    return "Signed in to Sikemux";
}

/** Why this host is signed out, when the account let it go rather than the person here. */
function removalNote(link: AccountLink | null): string | null {
    if (link?.state !== "removed") return null;
    if (link.reason === "account_deleted") return "Your account was deleted. Phones already allowed stay allowed.";
    if (link.reason === "signed_out") return "This computer was signed out of your account. Phones already allowed stay allowed.";
    return "This computer was removed from your account at app.sikemux.com. Phones already allowed stay allowed.";
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

/** Loads the account, and loads it again when the core says the account let this host go. */
function useSignedInAccount(link: AccountLink | null): AccountStatus | null {
    const account = useAccount((s) => s.account);
    const removed = link?.state === "removed";
    useEffect(() => {
        loadAccount().catch(reportError("Account"));
    }, []);
    useEffect(() => {
        if (removed && account?.signedIn) loadAccount().catch(reportError("Account"));
    }, [removed, account?.signedIn]);
    return account;
}

export function DevicesPage() {
    const [status, apply] = useRemoteStatus();
    const link = status?.account ?? null;
    const account = useSignedInAccount(link);
    const signedIn = account?.signedIn ?? false;
    const devices = status?.devices ?? [];
    const now = Date.now();

    return (
        <SettingsPage>
            <div className="devices-top">
                {!account ? (
                    <DevicesRow target="Your account" mark={<span className="devices-avatar" />} title="Your account" desc="Checking…">
                        {null}
                    </DevicesRow>
                ) : (
                    signedIn && <AccountRow account={account} link={link} />
                )}
                <DevicesRow
                    target="Remote access"
                    mark={<IconGlobe size={18} />}
                    title="Remote access"
                    desc={signedIn ? "Phones on your account can ask to connect. You allow each one." : "Sign in to use Sikemux on your phone"}
                    asLabel>
                    <Switch
                        checked={status?.enabled ?? false}
                        disabled={!status || !signedIn}
                        onChange={(enabled) => void apply(remoteApi.setEnabled(enabled), "Remote access")}
                        label="Remote access"
                    />
                </DevicesRow>
            </div>

            {!account ? null : !signedIn ? <SignInCall removed={removalNote(link)} /> : status && devices.length === 0 ? <PhonePanel /> : null}

            {(!account || signedIn || devices.length > 0) && (
                <section className="devices-phones" data-settings-target="Phones">
                    <h2 className="devices-label">Phones</h2>
                    {!status ? null : devices.length === 0 ? (
                        <p className="devices-waiting">
                            <span className="devices-waiting-loader agent-state-loader" aria-hidden="true">
                                {Array.from({ length: 9 }, (_, index) => (
                                    <i key={index} />
                                ))}
                            </span>
                            None yet. Waiting for your phone.
                        </p>
                    ) : (
                        <SettingsRows>
                            {devices.map((device) => (
                                <DeviceRow
                                    key={device.id}
                                    device={device}
                                    connected={status?.connected.includes(device.id) ?? false}
                                    notifications={status?.notifications.find((phone) => phone.deviceId === device.id)?.state}
                                    now={now}
                                    onAccess={(access) => void apply(remoteApi.setDeviceAccess(device.id, access), "Device access")}
                                    onRevoke={() => void apply(remoteApi.revokeDevice(device.id), "Remove phone")}
                                />
                            ))}
                        </SettingsRows>
                    )}
                </section>
            )}
        </SettingsPage>
    );
}

function DevicesRow({
    target,
    mark,
    title,
    desc,
    asLabel = false,
    children,
}: {
    target: string;
    mark: ReactNode;
    title: string;
    desc: string;
    asLabel?: boolean;
    children: ReactNode;
}) {
    const Tag = asLabel ? "label" : "div";
    return (
        <Tag className="devices-row" data-settings-target={target}>
            <span className="devices-row-mark">{mark}</span>
            <span className="devices-row-copy">
                <span className="devices-row-title">{title}</span>
                <span className="devices-row-desc">{desc}</span>
            </span>
            <span className="devices-row-control">{children}</span>
        </Tag>
    );
}

function AccountRow({ account, link }: { account: AccountStatus; link: AccountLink | null }) {
    const [leaving, setLeaving] = useState(false);
    const signOut = async () => {
        setLeaving(true);
        try {
            setAccount(await accountApi.signOut());
        } catch (error) {
            reportError("Sign out")(error);
        } finally {
            setLeaving(false);
        }
    };
    return (
        <DevicesRow
            target="Your account"
            mark={<AccountAvatar account={account} className="devices-avatar" />}
            title={account.email ?? account.name ?? "Signed in"}
            desc={accountLine(link)}>
            <span className="settings-actions">
                <button
                    className="settings-btn"
                    type="button"
                    title="Delete your account at app.sikemux.com"
                    onClick={() => void portsApi.openExternal(DELETE_ACCOUNT_URL).catch(reportError("Open link"))}>
                    Delete account…
                </button>
                <button className="settings-btn" type="button" disabled={leaving} onClick={() => void signOut()}>
                    {leaving ? "Signing out…" : "Sign out"}
                </button>
            </span>
        </DevicesRow>
    );
}

function SignInCall({ removed }: { removed: string | null }) {
    const [waiting, setWaiting] = useState(false);
    const signIn = async () => {
        setWaiting(true);
        try {
            setAccount(await accountApi.signIn());
            cmd.openSettings("devices");
            offerAccountPhones().catch(swallow("phones on the account"));
        } catch (error) {
            if (!String(error).includes("cancelled")) reportError("Sign in")(error);
        } finally {
            setWaiting(false);
        }
    };
    return (
        <section className="devices-panel devices-sign-in" data-settings-target="Your account">
            <div className="devices-panel-copy">
                <h3 className="devices-panel-title">{waiting ? "Finish signing in in your browser" : "Sign in to your Sikemux account"}</h3>
                <p className="devices-panel-lede">
                    {waiting
                        ? "Sikemux opened the sign-in page in your default browser."
                        : (removed ?? "Your phone finds this computer through it. Sign in with Google, GitHub or your email, in your browser.")}
                </p>
                <div className="devices-panel-actions">
                    {waiting ? (
                        <button className="settings-btn" type="button" onClick={() => void accountApi.cancelSignIn()}>
                            Cancel
                        </button>
                    ) : (
                        <button className="settings-btn primary" type="button" onClick={() => void signIn()}>
                            Sign in
                        </button>
                    )}
                </div>
            </div>
        </section>
    );
}

function PhonePanel() {
    return (
        <section className="devices-panel devices-phone" aria-label="Your agents, on your phone">
            <div className="devices-panel-copy">
                <h3 className="devices-panel-title">Your agents, on your phone</h3>
                <p className="devices-panel-lede">Watch them work, answer what they ask and start new chats from anywhere.</p>
                <ol className="devices-steps">
                    <li>Scan the code with your phone&apos;s camera</li>
                    <li>Sign in with the same account</li>
                    <li>Click Allow here when it asks</li>
                </ol>
            </div>
            <span className="devices-qr">
                <QrCode text={PHONE_URL} label={`QR code for ${PHONE_URL}`} />
            </span>
        </section>
    );
}

function DeviceRow({
    device,
    connected,
    notifications,
    now,
    onAccess,
    onRevoke,
}: {
    device: PairedDevice;
    connected: boolean;
    notifications: NotificationState | undefined;
    now: number;
    onAccess: (access: DeviceAccess) => void;
    onRevoke: () => void;
}) {
    const note = notificationNote(notifications);
    const seen = connected ? "connected now" : seenLabel(device.lastSeen, now);
    return (
        <SettingsRow
            label={device.name || "Unnamed phone"}
            desc={
                <>
                    {platformName(device.platform)} · {seen}
                    {note && (
                        <>
                            {" · "}
                            <span className={notifications === "notReaching" || notifications === "otherAccount" ? "device-warning" : undefined}>
                                {note}
                            </span>
                        </>
                    )}
                </>
            }>
            <span className="device-controls">
                <Dropdown
                    className="settings-dd"
                    label={`access for ${device.name}`}
                    value={device.access}
                    options={ACCESS_OPTIONS}
                    onChange={(value) => onAccess(value as DeviceAccess)}
                />
                <button className="settings-btn danger" type="button" onClick={onRevoke} title="Forget this phone and end its connections">
                    <IconTrash size={12} /> Remove
                </button>
            </span>
        </SettingsRow>
    );
}
