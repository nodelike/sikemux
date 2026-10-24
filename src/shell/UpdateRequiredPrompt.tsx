import { useEffect, useState } from "react";
import { exit } from "@tauri-apps/plugin-process";
import { remoteApi, type UpdateRequired } from "../api/remote";
import { openUrl } from "../api/system";
import {
    checkForUpdate,
    checkForUpdateNow,
    installPendingUpdate,
    isUpdateBusy,
    RELEASES_URL,
    UPDATES_IN_APP,
    updateStatusLabel,
} from "../api/updater";
import { useOccludeNativeViews } from "../state/nativeViews";
import { canFlushPersist, flushPersist } from "../state/persist";
import { useStore } from "../state/store";
import { swallow } from "../state/toast";
import { updateChannelOf } from "../state/preferences";

function useUpdateRequired(): UpdateRequired | null {
    const [required, setRequired] = useState<UpdateRequired | null>(null);
    useEffect(() => {
        const controller = new AbortController();
        remoteApi
            .subscribe((status) => setRequired(status.updateRequired), controller.signal)
            .then(() => remoteApi.status())
            .then((status) => {
                if (!controller.signal.aborted) setRequired(status.updateRequired);
            })
            .catch((error: unknown) => {
                if (!controller.signal.aborted) swallow("remote access status")(error);
            });
        return () => controller.abort();
    }, []);
    return required;
}

async function quit(): Promise<void> {
    if (canFlushPersist()) await flushPersist().catch(() => false);
    await exit(0);
}

/** Covers the app while the accounts server no longer works with this build. The only ways on are updating or quitting. */
export function UpdateRequiredPrompt() {
    const required = useUpdateRequired();
    if (!required) return null;
    return <Prompt required={required} />;
}

function Prompt({ required }: { required: UpdateRequired }) {
    const pending = useStore((s) => s.pendingUpdate);
    const channel = useStore(updateChannelOf);
    const checked = useStore((s) => s.lastUpdateCheck);
    useOccludeNativeViews(true);
    useEffect(() => {
        if (UPDATES_IN_APP) void checkForUpdate();
    }, []);

    const busy = pending ? isUpdateBusy(pending.state) : false;
    const nothingNewer = !pending && checked !== null && checked.error === null;
    return (
        <div className="experience-backdrop" role="presentation">
            <section className="update-required" role="alertdialog" aria-modal="true" aria-labelledby="update-required-title">
                <h1 id="update-required-title">Update Sikemux</h1>
                <p>
                    This version, {required.current}, is older than Sikemux&apos;s servers work with now. Remote access and your account stay off
                    until you update to {required.minimum} or later.
                </p>
                {pending?.state === "error" && <p className="update-required-problem">{pending.error}</p>}
                {checked?.error && !pending && <p className="update-required-problem">{checked.error}</p>}
                {nothingNewer && (
                    <p className="update-required-note">
                        No newer version on the {channel} channel yet.{" "}
                        <button
                            type="button"
                            className="update-required-link"
                            onClick={() => void openUrl(RELEASES_URL).catch(swallow("open releases"))}>
                            See every release
                        </button>
                    </p>
                )}
                <div className="update-required-actions">
                    <button type="button" onClick={() => void quit()} disabled={busy}>
                        Quit Sikemux
                    </button>
                    {!UPDATES_IN_APP ? (
                        <button type="button" className="primary" onClick={() => void openUrl(RELEASES_URL).catch(swallow("open releases"))}>
                            Download the new version
                        </button>
                    ) : pending ? (
                        <button type="button" className="primary" onClick={() => void installPendingUpdate()} disabled={busy}>
                            {busy || pending.state === "error" ? updateStatusLabel(pending) : `Update to v${pending.version}`}
                        </button>
                    ) : (
                        <button type="button" className="primary" onClick={() => void checkForUpdateNow()}>
                            Check for updates
                        </button>
                    )}
                </div>
            </section>
        </div>
    );
}
