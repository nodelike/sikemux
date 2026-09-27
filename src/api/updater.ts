import { Channel } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { invokeCommand as invoke } from "./invoke";
import type { ReleaseCredits } from "./releases";
import { getState, setState, type PendingUpdate, type UpdateCheckOutcome, type UpdateOperationState } from "../state/store";
import { errMessage, notify, reportError, swallow } from "../state/toast";

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

let activeInstall: Promise<void> | null = null;

export function isUpdateBusy(state: UpdateOperationState | undefined): boolean {
    return state !== undefined && BUSY_UPDATE_STATES.has(state);
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

function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function updateStatusLabel(pending: PendingUpdate): string {
    switch (pending.state) {
        case "available":
            return `Update · v${pending.version}`;
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

// A user-initiated check reports its outcome; the periodic one stays quiet so a
// flaky network cannot spam toasts. Either way the attempt is recorded, because
// a check that only fails into a swallowed error is indistinguishable from
// being up to date.
async function runUpdateCheck(announce: boolean): Promise<void> {
    if (isUpdateBusy(getState().pendingUpdate?.state)) return;
    const channel = getState().updateChannel;
    try {
        const update = await invoke<UpdateInfo | null>("update_check", { channel });
        setState({ lastUpdateCheck: { at: Date.now(), channel, error: null } });
        // The periodic check may have started just before an install. Never let
        // its stale response erase newer operation progress.
        if (isUpdateBusy(getState().pendingUpdate?.state)) return;
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
        setState({ lastUpdateCheck: { at: Date.now(), channel, error: errMessage(error) } });
        if (announce) reportError("update check")(error);
        else swallow("update check")(error);
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

async function installPendingUpdateOnce(): Promise<void> {
    if (!getState().pendingUpdate) await checkForUpdate();
    const pending = getState().pendingUpdate;
    if (!pending || isUpdateBusy(pending.state)) return;
    const channel = getState().updateChannel;

    setState((state) => ({
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
        setState((state) => ({
            pendingUpdate: state.pendingUpdate
                ? {
                      ...state.pendingUpdate,
                      state: "restarting",
                      error: null,
                      downloadedBytes: state.pendingUpdate.totalBytes ?? state.pendingUpdate.downloadedBytes,
                  }
                : null,
            lastReleaseNotes: { version: installed.version, notes: installed.notes, date: installed.date, credits: installed.credits },
        }));
        await relaunch();
    } catch (error) {
        setState((state) => ({
            pendingUpdate: state.pendingUpdate ? { ...state.pendingUpdate, state: "error", error: String(error) } : null,
        }));
    }
}

export function installPendingUpdate(): Promise<void> {
    if (activeInstall) return activeInstall;
    const operation = installPendingUpdateOnce();
    const tracked = operation.finally(() => {
        if (activeInstall === tracked) activeInstall = null;
    });
    activeInstall = tracked;
    return tracked;
}
