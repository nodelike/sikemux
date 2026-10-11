import { useRef, useState } from "react";
import { invalidate } from "../../../plugin-api/resources";
import { Checkbox, IconChevron } from "../../../plugin-api/ui";
import { errorMessage, rundeckApi, type RundeckStatus } from "../api";
import * as cmd from "../state";
import { DEFAULT_BRANCH_OPTIONS, DEFAULT_PROD_ENVS, splitList } from "../shape";
import { useMenuKeys } from "./hooks";
import { hostFromUrl, Status } from "./parts";

/** The server card at the foot of the sidebar; it opens the plugin's settings above itself. */
export function RundeckConnection({ status, onSignedOut }: { status: RundeckStatus; onSignedOut: () => void }) {
    const [open, setOpen] = useState(false);
    const panelRef = useRef<HTMLDivElement>(null);
    useMenuKeys(open, panelRef, () => setOpen(false), false);

    return (
        <div className="rnd-conn-wrap">
            {open && (
                <>
                    <div className="rnd-pop-scrim" onClick={() => setOpen(false)} />
                    <div className="rnd-pop" role="dialog" aria-label="Rundeck settings" ref={panelRef}>
                        <SettingsForm
                            onSignedOut={() => {
                                setOpen(false);
                                onSignedOut();
                            }}
                        />
                    </div>
                </>
            )}
            <button
                className={`rnd-conn${open ? " open" : ""}`}
                aria-haspopup="dialog"
                aria-expanded={open}
                onClick={() => setOpen((v) => !v)}
                title="Rundeck settings">
                <span className="rnd-conn-top">
                    <span className="rnd-conn-host">{hostFromUrl(status.url)}</span>
                    <IconChevron size={11} className="rnd-conn-chev" />
                </span>
                <span className="rnd-conn-sub">
                    {[status.user, status.rundeck_version && `Rundeck ${status.rundeck_version}`].filter(Boolean).join(" · ")}
                </span>
                <span className="rnd-conn-meta">
                    <span>{status.token_present ? "API token" : "No token"}</span>
                    <Status status={status.ok ? "succeeded" : "failed"} label={status.ok ? "Connected" : "Offline"} />
                </span>
            </button>
        </div>
    );
}

function SettingsForm({ onSignedOut }: { onSignedOut: () => void }) {
    const prodEnvs = cmd.rundeckSettings.useSelect((s) => s.prodEnvs);
    const branchOptions = cmd.rundeckSettings.useSelect((s) => s.branchOptions);
    const notifyWhenDone = cmd.rundeckSettings.useSelect((s) => s.notifyWhenDone);
    const [signingOut, setSigningOut] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const signOut = async () => {
        setSigningOut(true);
        setError(null);
        try {
            await rundeckApi.logout();
            invalidate((kind) => kind.startsWith("rnd."));
            onSignedOut();
        } catch (e) {
            setError(errorMessage(e));
        } finally {
            setSigningOut(false);
        }
    };

    return (
        <div className="rnd-pop-body">
            <ListField
                autoFocus
                label="Production folders"
                help="A job counts as production when its top folder or project name contains one of these words."
                value={prodEnvs}
                fallback={DEFAULT_PROD_ENVS}
                onCommit={(list) => cmd.updateRundeckSettings({ prodEnvs: list })}
            />
            <ListField
                label="Branch options"
                help="Job options that hold the git branch, tried in order, in any case."
                value={branchOptions}
                fallback={DEFAULT_BRANCH_OPTIONS}
                onCommit={(list) => {
                    cmd.updateRundeckSettings({ branchOptions: list });
                    invalidate((kind) => kind === "rnd.matrix" || kind === "rnd.jobCells" || kind === "rnd.plan");
                }}
            />
            <div className="rnd-field">
                <Checkbox checked={notifyWhenDone} onChange={(on) => cmd.updateRundeckSettings({ notifyWhenDone: on })}>
                    Tell me when a run I start finishes or fails
                </Checkbox>
                <small className="rnd-field-help">A notification arrives even while Sikemux is in the background.</small>
            </div>
            {error && <div className="rnd-field-error">{error}</div>}
            <div className="rnd-pop-foot">
                <button className="rnd-btn rnd-btn-danger" onClick={() => void signOut()} disabled={signingOut}>
                    {signingOut ? "Signing out…" : "Sign out"}
                </button>
            </div>
        </div>
    );
}

function ListField({
    autoFocus = false,
    label,
    help,
    value,
    fallback,
    onCommit,
}: {
    autoFocus?: boolean;
    label: string;
    help: string;
    value: string[];
    fallback: string[];
    onCommit: (list: string[]) => void;
}) {
    const [text, setText] = useState(value.join(", "));
    const commit = () => {
        const list = splitList(text);
        const next = list.length ? list : fallback;
        setText(next.join(", "));
        if (next.join("\n") !== value.join("\n")) onCommit(next);
    };
    return (
        <label className="rnd-field">
            <span>{label}</span>
            <input
                type="text"
                autoFocus={autoFocus}
                value={text}
                onChange={(e) => setText(e.target.value)}
                onBlur={commit}
                onKeyDown={(e) => {
                    if (e.key === "Enter") commit();
                }}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
            />
            <small className="rnd-field-help">{help}</small>
        </label>
    );
}
