import { Channel } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { invokeCommand as invoke } from "./invoke";
import type { ReleaseCredits } from "./releases";
import { getState, setState, type PendingUpdate, type UpdateBlocker, type UpdateCheckOutcome, type UpdateOperationState } from "../state/store";
import { notify, reportError, swallow } from "../state/toast";
import { errorCategory, errorText, formatBytes } from "../lib/format";

export const RELEASES_URL = "https://github.com/nodelike/sikemux/releases";
// The update feed only lists macOS builds, so Linux installs update by downloading the next release.
export const UPDATES_IN_APP = !/^Linux/i.test(typeof navigator === "undefined" ? "" : navigator.platform);

interface UpdateInfo {
    version: string;
    currentVersion: string;
    notes: string | null;
    date: string | null;
    credits: ReleaseCredits | null;
}

export interface UpdateInstallProgress {
    phase: "downloading" | "installing" | "installed";
    downloadedBytes: number;
    totalBytes: number | null;
}

const BUSY_UPDATE_STATES: ReadonlySet<UpdateOperationState> = new Set(["preparing", "downloading", "installing", "restarting"]);
const BLOCKER_POLL_MS = 2_000;

let activeInstall: Promise<void> | null = null;
let blockerPoll: ReturnType<typeof setTimeout> | null = null;
// The update is on disk and only the restart into it waits on agents.
let installedAwaitingRestart = false;

export function isUpdateBusy(state: UpdateOperationState | undefined): boolean {
    return state !== undefined && BUSY_UPDATE_STATES.has(state);
}

/** An install under way, or one waiting on agents to finish starting. */
function isUpdateUnderway(state: UpdateOperationState | undefined): boolean {
    return state === "waiting" || isUpdateBusy(state);
}

export function waitingLabel(count: number): string {
    return `Update waiting on ${count} agent${count === 1 ? "" : "s"}`;
}

function validByteCount(value: unknown): value is number {
    return Number.isSafeInteger(value) && (value as number) >= 0;
}

export function parseUpdateInstallProgress(value: unknown): UpdateInstallProgress | null {
    if (typeof value !== "object" || value === null) return null;
    const record = value as Record<string, unknown>;
    if (record.phase !== "downloading" && record.phase !== "installing" && record.phase !== "installed") return null;
    if (!validByteCount(record.downloadedBytes)) return null;
    if (record.totalBytes !== null && !validByteCount(record.totalBytes)) return null;
    return {
        phase: record.phase,
        downloadedBytes: record.downloadedBytes,
        totalBytes: record.totalBytes,
    };
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

function applyInstallProgress(rawProgress: unknown): void {
    const progress = parseUpdateInstallProgress(rawProgress);
    if (!progress) return;
    setState((state) => {
        if (!state.pendingUpdate || !isUpdateBusy(state.pendingUpdate.state)) return {};
        const nextState: UpdateOperationState =
            progress.phase === "downloading" ? "downloading" : progress.phase === "installing" ? "installing" : "restarting";
        return {
            pendingUpdate: {
                ...state.pendingUpdate,
                state: nextState,
                downloadedBytes: progress.downloadedBytes,
                totalBytes: progress.totalBytes,
            },
        };
    });
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

// A user-initiated check reports its outcome; the periodic one stays quiet so a
// flaky network cannot spam toasts. Either way the attempt is recorded, because
// a check that only fails into a swallowed error is indistinguishable from
// being up to date.
async function runUpdateCheck(announce: boolean): Promise<void> {
    if (isUpdateUnderway(getState().pendingUpdate?.state)) return;
    const channel = getState().updateChannel;
    try {
        const update = await invoke<UpdateInfo | null>("update_check", { channel });
        setState({ lastUpdateCheck: { at: Date.now(), channel, error: null } });
        // The periodic check may have started just before an install. Never let
        // its stale response erase newer operation progress.
        if (isUpdateUnderway(getState().pendingUpdate?.state)) return;
        if (!update) {
            setState({ pendingUpdate: null });
            if (announce) notify("info", `Sikemux is up to date on the ${channel} channel.`);
            return;
        }
        setState({
            pendingUpdate: {
                version: update.version,
                currentVersion: update.currentVersion,
                notes: update.notes,
                date: update.date,
                credits: update.credits,
                state: "available",
                error: null,
                downloadedBytes: 0,
                totalBytes: null,
            },
        });
        if (announce) notify("success", `Update v${update.version} is ready to install.`);
    } catch (error) {
        setState({ lastUpdateCheck: { at: Date.now(), channel, error: errorText(error) } });
        if (!announce) swallow("update check")(error);
        else if (isOffline(error)) notify("info", "You're offline. Sikemux will check for updates when you're back.");
        else reportError("update check")(error);
    }
}

export function checkForUpdate(): Promise<void> {
    return runUpdateCheck(false);
}

let activeCheck: Promise<void> | null = null;

/** User-initiated check. Reports failures instead of swallowing them. */
export function checkForUpdateNow(): Promise<void> {
    if (activeCheck) return activeCheck;
    const operation = runUpdateCheck(true);
    const tracked = operation.finally(() => {
        if (activeCheck === tracked) activeCheck = null;
    });
    activeCheck = tracked;
    return tracked;
}

/** The chats an update now would stop rather than carry over. A core that
 *  cannot say holds nothing up. */
async function updateBlockers(): Promise<UpdateBlocker[]> {
    try {
        return await invoke<UpdateBlocker[]>("update_blockers");
    } catch (error) {
        swallow("update blockers")(error);
        return [];
    }
}

function setWaiting(blockers: UpdateBlocker[]): void {
    setState((state) => ({
        updateBlockers: blockers,
        pendingUpdate: state.pendingUpdate ? { ...state.pendingUpdate, state: "waiting", error: null } : null,
    }));
}

function pollBlockers(): void {
    if (blockerPoll) return;
    blockerPoll = setTimeout(() => {
        blockerPoll = null;
        if (getState().pendingUpdate?.state !== "waiting") return;
        void updateBlockers().then((blockers) => {
            if (getState().pendingUpdate?.state !== "waiting") return;
            if (blockers.length === 0) {
                void installPendingUpdate();
                return;
            }
            setState({ updateBlockers: blockers });
            pollBlockers();
        });
    }, BLOCKER_POLL_MS);
}

function setRestarting(): void {
    setState((state) => ({
        updateBlockers: [],
        pendingUpdate: state.pendingUpdate
            ? {
                  ...state.pendingUpdate,
                  state: "restarting",
                  error: null,
                  downloadedBytes: state.pendingUpdate.totalBytes ?? state.pendingUpdate.downloadedBytes,
              }
            : null,
    }));
}

/** Restarts into the installed update. Agents that became busy and can't be
 *  carried over since the download hold the restart, unless forced; forcing
 *  tells the core to stop them rather than wait. */
async function restartIntoUpdate(force: boolean): Promise<void> {
    if (force) {
        await invoke("update_force_core");
    } else {
        const blockers = await updateBlockers();
        if (blockers.length > 0) {
            installedAwaitingRestart = true;
            setWaiting(blockers);
            pollBlockers();
            return;
        }
    }
    installedAwaitingRestart = false;
    setRestarting();
    await relaunch();
}

async function installPendingUpdateOnce(force: boolean): Promise<void> {
    if (!getState().pendingUpdate) await checkForUpdate();
    const pending = getState().pendingUpdate;
    if (!pending || isUpdateBusy(pending.state)) return;
    const channel = getState().updateChannel;

    if (installedAwaitingRestart) {
        try {
            await restartIntoUpdate(force);
        } catch (error) {
            setState((state) => ({
                pendingUpdate: state.pendingUpdate ? { ...state.pendingUpdate, state: "error", error: errorText(error) } : null,
            }));
        }
        return;
    }

    // Agents at work are carried over the update still running. One that is
    // still starting cannot be, so the install waits for it unless forced.
    if (!force) {
        const blockers = await updateBlockers();
        if (blockers.length > 0) {
            setWaiting(blockers);
            pollBlockers();
            return;
        }
    }

    setState((state) => ({
        updateBlockers: [],
        pendingUpdate: state.pendingUpdate
            ? {
                  ...state.pendingUpdate,
                  state: "preparing",
                  error: null,
                  downloadedBytes: 0,
                  totalBytes: null,
              }
            : null,
    }));

    const onProgress = new Channel<UpdateInstallProgress>();
    onProgress.onmessage = applyInstallProgress;

    try {
        const installed = await invoke<UpdateInfo>("update_install", {
            channel,
            onProgress,
        });
        // Commit release bookkeeping only after the signed artifact was
        // installed successfully. Keep the restart state visible if relaunch
        // itself fails instead of making the update silently disappear.
        setState({
            lastReleaseNotes: { version: installed.version, notes: installed.notes, date: installed.date, credits: installed.credits },
        });
        setRestarting();
        await restartIntoUpdate(force);
    } catch (error) {
        setState((state) => ({
            pendingUpdate: state.pendingUpdate ? { ...state.pendingUpdate, state: "error", error: errorText(error) } : null,
        }));
    }
}

/** Installs now, stopping the agents the update would otherwise wait on;
 *  they start again on their sessions once Sikemux is back. */
export function forceUpdate(): Promise<void> {
    if (activeInstall) return activeInstall;
    const operation = installPendingUpdateOnce(true);
    const tracked = operation.finally(() => {
        if (activeInstall === tracked) activeInstall = null;
    });
    activeInstall = tracked;
    return tracked;
}

/** Stops waiting on agents; the update stays available to install later. */
export function cancelWaitingUpdate(): void {
    setState((state) => ({
        updateBlockers: [],
        pendingUpdate: state.pendingUpdate?.state === "waiting" ? { ...state.pendingUpdate, state: "available" } : state.pendingUpdate,
    }));
}

export function installPendingUpdate(): Promise<void> {
    if (activeInstall) return activeInstall;
    const operation = installPendingUpdateOnce(false);
    const tracked = operation.finally(() => {
        if (activeInstall === tracked) activeInstall = null;
    });
    activeInstall = tracked;
    return tracked;
}
