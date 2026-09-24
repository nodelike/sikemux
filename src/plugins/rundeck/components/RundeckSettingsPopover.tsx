import { useRef, useState } from "react";
import { invalidate } from "../../../plugin-api/resources";
import { Tooltip } from "../../../plugin-api/ui";
import { errorMessage, rundeckApi } from "../api";
import * as cmd from "../state";
import { DEFAULT_BRANCH_OPTIONS, DEFAULT_PROD_ENVS, splitList } from "../shape";
import { useMenuKeys } from "./hooks";

export function RundeckSettingsPopover({ onSignedOut }: { onSignedOut: () => void }) {
    const [open, setOpen] = useState(false);
    const panelRef = useRef<HTMLDivElement>(null);
    useMenuKeys(open, panelRef, () => setOpen(false), false);

    return (
        <span className="rnd-pop-anchor">
            <Tooltip label="Rundeck settings">
                <button
                    className={`rnd-bar-icon${open ? " on" : ""}`}
                    aria-label="Rundeck settings"
                    aria-haspopup="dialog"
                    aria-expanded={open}
                    onClick={() => setOpen((v) => !v)}>
                    <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true">
                        <path d="M2 4h7M12 4h2M2 12h2M7 12h7" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" fill="none" />
                        <circle cx="10.5" cy="4" r="1.6" stroke="currentColor" strokeWidth="1.3" fill="none" />
                        <circle cx="5.5" cy="12" r="1.6" stroke="currentColor" strokeWidth="1.3" fill="none" />
                    </svg>
                </button>
            </Tooltip>
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
        </span>
    );
}

function SettingsForm({ onSignedOut }: { onSignedOut: () => void }) {
    const prodEnvs = cmd.rundeckSettings.useSelect((s) => s.prodEnvs);
    const branchOptions = cmd.rundeckSettings.useSelect((s) => s.branchOptions);
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
                label="production environments"
                help="A job is production when its top folder or project name contains one of these words."
                value={prodEnvs}
                fallback={DEFAULT_PROD_ENVS}
                onCommit={(list) => cmd.updateRundeckSettings({ prodEnvs: list })}
            />
            <ListField
                label="branch options"
                help="Job options that hold the git branch, tried in order, any case."
                value={branchOptions}
                fallback={DEFAULT_BRANCH_OPTIONS}
                onCommit={(list) => {
                    cmd.updateRundeckSettings({ branchOptions: list });
                    invalidate((kind) => kind === "rnd.matrix" || kind === "rnd.jobCells" || kind === "rnd.plan");
                }}
            />
            {error && <div className="rnd-field-error">{error}</div>}
            <div className="rnd-pop-foot">
                <button className="rnd-btn rnd-btn-danger" onClick={() => void signOut()} disabled={signingOut}>
                    {signingOut ? "signing out…" : "sign out"}
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
