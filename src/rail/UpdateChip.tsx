import { isUpdateBusy, updateDownloadPercent, updateStatusLabel } from "../api/updater";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { IconDownload, IconRefresh, IconWarning } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";

export function UpdateChip() {
    const pending = useStore((s) => s.pendingUpdate);
    if (!pending) return null;

    const state = pending.state;
    const busy = isUpdateBusy(state);
    const statusLabel = updateStatusLabel(pending);
    const percent = state === "downloading" ? updateDownloadPercent(pending) : null;
    const label =
        state === "error"
            ? `Update v${pending.version} failed — ${pending.error ?? "unknown"}. Click to retry.`
            : busy
              ? `${statusLabel} v${pending.version}`
              : `Update v${pending.version} available (current: v${pending.currentVersion}). Click to install + relaunch.${pending.notes ? `\n\n${pending.notes}` : ""}`;
    const Glyph = state === "error" ? IconWarning : state === "installing" || state === "restarting" ? IconRefresh : IconDownload;

    return (
        <Tooltip label={label}>
            <button
                className={`rail-update rail-update-${state}${percent === null ? "" : " rail-update-measured"}`}
                onClick={cmd.openWhatsNew}
                disabled={busy}
                aria-label={label}>
                {percent !== null && <span className="rail-update-fill" style={{ transform: `scaleX(${percent / 100})` }} aria-hidden="true" />}
                <Glyph size={12} />
                <span className="rail-update-label">{statusLabel}</span>
            </button>
        </Tooltip>
    );
}
