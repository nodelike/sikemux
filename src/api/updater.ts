import { coreCall } from "./core";
import { onCoreChanged } from "./coreChanged";
import type { UpdateState, UpdateStatus } from "@shared/wire.generated";
import { invokeCommand as invoke } from "./invoke";
import { getState, setState, type PendingUpdate, type UpdateCheckOutcome, type UpdateOperationState } from "../state/store";
import { notify, reportError, swallow } from "../state/toast";
import { errorCategory, errorText, formatBytes } from "@shared/lib/format";

export const RELEASES_URL = "https://github.com/nodelike/sikemux/releases";
// The update feed only lists macOS builds, so Linux installs update by downloading the next release.
export const UPDATES_IN_APP = !/^Linux/i.test(typeof navigator === "undefined" ? "" : navigator.platform);

const BUSY_UPDATE_STATES: ReadonlySet<UpdateOperationState> = new Set(["preparing", "downloading", "installing", "restarting"]);

const SHOWN_STATE: Record<UpdateState, UpdateOperationState> = {
    idle: "available",
    waiting: "waiting",
    downloading: "downloading",
    installing: "installing",
    ready: "restarting",
    failed: "error",
};

let restarting = false;

export function isUpdateBusy(state: UpdateOperationState | undefined): boolean {
    return state !== undefined && BUSY_UPDATE_STATES.has(state);
}

export function waitingLabel(count: number): string {
    return `Update waiting on ${count} agent${count === 1 ? "" : "s"}`;
}

export function updateDownloadPercent(pending: Pick<PendingUpdate, "downloadedBytes" | "totalBytes">): number | null {
    if (!pending.totalBytes || pending.totalBytes <= 0) return null;
    return Math.min(100, Math.max(0, Math.floor((pending.downloadedBytes / pending.totalBytes) * 100)));
}

export function updateStatusLabel(pending: PendingUpdate, waitingOn = 0): string {
    switch (pending.state) {
        case "available":
            return `Update · v${pending.version}`;
        case "waiting":
            return waitingLabel(waitingOn);
        case "preparing":
            return "Preparing update…";
        case "downloading": {
            const percent = updateDownloadPercent(pending);
            if (percent !== null) return `Downloading · ${percent}%`;
            return pending.downloadedBytes > 0 ? `Downloading · ${formatBytes(pending.downloadedBytes)}` : "Downloading…";
        }
        case "installing":
            return "Verifying & installing…";
        case "restarting":
            return "Restarting…";
        case "error":
            return "Update failed — retry";
    }
}

export function updateCheckLabel(outcome: UpdateCheckOutcome): string {
    const when = new Date(outcome.at).toLocaleTimeString();
    if (outcome.error) return `Last ${outcome.channel} check failed at ${when} — ${outcome.error}`;
    return `Checked ${outcome.channel} at ${when}.`;
}

function isOffline(error: unknown): boolean {
    const category = errorCategory(error);
    return category === "offline" || category === "timeout";
}

/** Starts the app again into the update the core put in place, telling the
 *  next core to stop the chats it would otherwise wait on when the person forced it. */
async function restartInto(status: UpdateStatus): Promise<void> {
    if (restarting) return;
    restarting = true;
    try {
        if (status.forced) await invoke("update_force_core");
        await invoke("update_restart");
    } catch (error) {
        restarting = false;
        setState((state) => ({
            pendingUpdate: state.pendingUpdate ? { ...state.pendingUpdate, state: "error", error: errorText(error) } : null,
        }));
    }
}

/** Holds what the core says of updates as the store shows it. */
function show(status: UpdateStatus): void {
    const checked: UpdateCheckOutcome | null = status.checked && {
        at: status.checked.at,
        channel: status.checked.channel,
        error: status.checked.error,
    };
    const available = status.available;
    const pendingUpdate: PendingUpdate | null = available && {
        version: available.version,
        currentVersion: status.current,
        notes: available.notes,
        date: available.date,
        credits: available.credits,
        state: SHOWN_STATE[status.state],
        error: status.error,
        downloadedBytes: status.downloadedBytes,
        totalBytes: status.totalBytes,
    };
    setState({ lastUpdateCheck: checked, pendingUpdate, updateBlockers: status.state === "waiting" ? status.blockers : [] });
    if (status.state !== "ready" || !available) return;
    setState({ lastReleaseNotes: { version: available.version, notes: available.notes, date: available.date, credits: available.credits } });
    void restartInto(status);
}

function readStatus(): Promise<void> {
    return coreCall<UpdateStatus>("updateStatus").then(show, swallow("update status"));
}

/** Shows the core's updates as they move; the core checks on its own. */
export function followUpdates(): () => void {
    void readStatus();
    return onCoreChanged("update", () => void readStatus());
}

/** Asks the core to check now without telling the person how it went. */
export function checkForUpdate(): Promise<void> {
    return coreCall<UpdateStatus>("updateCheck").then(show, (error: unknown) => {
        swallow("update check")(error);
        return readStatus();
    });
}

let activeCheck: Promise<void> | null = null;

async function checkAndSay(): Promise<void> {
    try {
        const status = await coreCall<UpdateStatus>("updateCheck");
        show(status);
        const channel = status.checked?.channel ?? "stable";
        if (!status.available) notify("info", `Sikemux is up to date on the ${channel} channel.`);
        else if (status.state === "idle") notify("success", `Update v${status.available.version} is ready to install.`);
    } catch (error) {
        await readStatus();
        if (isOffline(error)) notify("info", "You're offline. Sikemux will check for updates when you're back.");
        else reportError("update check")(error);
    }
}

/** A check the person asked for, which says how it went. */
export function checkForUpdateNow(): Promise<void> {
    if (activeCheck) return activeCheck;
    const tracked = checkAndSay().finally(() => {
        if (activeCheck === tracked) activeCheck = null;
    });
    activeCheck = tracked;
    return tracked;
}

let activeInstall: Promise<void> | null = null;

async function install(force: boolean): Promise<void> {
    if (!getState().pendingUpdate) await checkForUpdate();
    const pending = getState().pendingUpdate;
    if (!pending || isUpdateBusy(pending.state)) return;
    setState((state) => ({
        pendingUpdate: state.pendingUpdate ? { ...state.pendingUpdate, state: "preparing", error: null, downloadedBytes: 0, totalBytes: null } : null,
    }));
    try {
        show(await coreCall<UpdateStatus>("updateInstall", { force }));
    } catch (error) {
        setState((state) => ({
            pendingUpdate: state.pendingUpdate ? { ...state.pendingUpdate, state: "error", error: errorText(error) } : null,
        }));
    }
}

function once(force: boolean): Promise<void> {
    if (activeInstall) return activeInstall;
    const tracked = install(force).finally(() => {
        if (activeInstall === tracked) activeInstall = null;
    });
    activeInstall = tracked;
    return tracked;
}

/** Downloads and installs the update, waiting on the agents it would stop. */
export function installPendingUpdate(): Promise<void> {
    return once(false);
}

/** Installs now, stopping the agents the update would otherwise wait on;
 *  they start again on their sessions once Sikemux is back. */
export function forceUpdate(): Promise<void> {
    return once(true);
}

/** Stops waiting on agents; the update stays available to install later. */
export function cancelWaitingUpdate(): void {
    void coreCall<UpdateStatus>("updateCancel").then(show, swallow("update cancel"));
}
