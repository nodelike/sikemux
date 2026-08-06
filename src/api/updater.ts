import { relaunch } from "@tauri-apps/plugin-process";
import { invoke } from "@tauri-apps/api/core";
import { getState, setState } from "../state/store";
import { swallow } from "../state/toast";

interface UpdateInfo {
    version: string;
    currentVersion: string;
    notes: string | null;
    date: string | null;
}

export async function checkForUpdate(): Promise<void> {
    try {
        const update = await invoke<UpdateInfo | null>("update_check", { channel: getState().updateChannel });
        if (!update) {
            setState({ pendingUpdate: null });
            return;
        }
        setState({
            pendingUpdate: {
                version: update.version,
                currentVersion: update.currentVersion,
                notes: update.notes,
                date: update.date,
                state: "available",
                error: null,
            },
        });
    } catch (error) {
        swallow("update check")(error);
    }
}

export async function installPendingUpdate(): Promise<void> {
    if (!getState().pendingUpdate) await checkForUpdate();
    if (!getState().pendingUpdate) return;

    setState((st) => ({
        pendingUpdate: st.pendingUpdate ? { ...st.pendingUpdate, state: "installing", error: null } : null,
        lastReleaseNotes: st.pendingUpdate
            ? { version: st.pendingUpdate.version, notes: st.pendingUpdate.notes, date: st.pendingUpdate.date }
            : st.lastReleaseNotes,
    }));

    try {
        await invoke("update_install", { channel: getState().updateChannel });
        setState({ pendingUpdate: null });
        await relaunch();
    } catch (e) {
        setState((st) => ({
            pendingUpdate: st.pendingUpdate ? { ...st.pendingUpdate, state: "error", error: String(e) } : null,
        }));
    }
}
