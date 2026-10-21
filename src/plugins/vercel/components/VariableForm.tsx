import { useState } from "react";
import { notify } from "../../../plugin-api/host";
import { Checkbox } from "../../../plugin-api/ui";
import { errorText } from "../../../plugin-api/format";
import { vercelApi, type VariableChange, type VercelTarget, type VercelVariable } from "../api";
import { vercel, vercelEnvR } from "../resources";

const TARGETS: { value: VercelTarget; label: string }[] = [
    { value: "production", label: "Production" },
    { value: "preview", label: "Preview" },
    { value: "development", label: "Development" },
];

const KEY = /^[A-Za-z_][A-Za-z0-9_]*$/;

export interface VariableDraft {
    key: string;
    value: string;
    targets: VercelTarget[];
    branch: string;
    sensitive: boolean;
}

/** What is wrong with the draft, in the words Vercel's form uses, or nothing. */
export function problemOf(draft: VariableDraft, editing: boolean): string | null {
    if (!KEY.test(draft.key.trim())) return "A name has letters, digits and _, and doesn't start with a digit.";
    if (!editing && draft.value === "") return "Enter a value.";
    if (draft.targets.length === 0) return "Pick at least one environment.";
    if (draft.branch.trim() && (draft.targets.length !== 1 || draft.targets[0] !== "preview")) return "A branch's variable is for Preview alone.";
    if (draft.sensitive && draft.targets.includes("development")) return "A sensitive variable can't be used in Development.";
    return null;
}

export function changeOf(draft: VariableDraft, editing: VercelVariable | null): VariableChange {
    return {
        ...(editing ? { id: editing.id } : { kind: draft.sensitive ? ("sensitive" as const) : ("encrypted" as const) }),
        key: draft.key.trim(),
        ...(draft.value === "" ? {} : { value: draft.value }),
        targets: draft.targets,
        ...(draft.branch.trim() ? { branch: draft.branch.trim() } : {}),
    };
}

/** Adds a variable, or changes one. The value is typed in, sent to Vercel and never shown again. */
export function VariableForm({
    team,
    project,
    editing,
    onDone,
}: {
    team: string;
    project: string;
    editing: VercelVariable | null;
    onDone: () => void;
}) {
    const [draft, setDraft] = useState<VariableDraft>({
        key: editing?.key ?? "",
        value: "",
        targets: (editing?.target as VercelTarget[] | undefined) ?? ["production", "preview", "development"],
        branch: editing?.gitBranch ?? "",
        sensitive: false,
    });
    const [saving, setSaving] = useState(false);
    const problem = problemOf(draft, editing !== null);
    const toggle = (target: VercelTarget, on: boolean) =>
        setDraft({ ...draft, targets: on ? [...draft.targets, target] : draft.targets.filter((each) => each !== target) });

    const save = async () => {
        setSaving(true);
        try {
            await vercelApi.setEnv(team, project, changeOf(draft, editing));
            vercel.refresh(vercelEnvR);
            notify("success", `${editing ? "Saved" : "Added"} ${draft.key.trim()}`);
            onDone();
        } catch (error) {
            notify("error", `${editing ? "Save" : "Add"} ${draft.key.trim()}: ${errorText(error)}`);
        } finally {
            setSaving(false);
        }
    };

    return (
        <form
            className="vercel-box vercel-form-box vercel-variable-form"
            aria-label={editing ? `Edit ${editing.key}` : "Add Environment Variable"}
            onSubmit={(event) => {
                event.preventDefault();
                if (!problem && !saving) void save();
            }}>
            <div className="vercel-variable-fields">
                <label className="vercel-form-stack">
                    <span>Key</span>
                    <input
                        className="vercel-find vercel-mono"
                        value={draft.key}
                        onChange={(event) => setDraft({ ...draft, key: event.target.value })}
                        placeholder="CLIENT_KEY"
                        spellCheck={false}
                        autoFocus={!editing}
                    />
                </label>
                <label className="vercel-form-stack">
                    <span>Value</span>
                    <input
                        className="vercel-find vercel-mono"
                        type="password"
                        autoComplete="new-password"
                        value={draft.value}
                        onChange={(event) => setDraft({ ...draft, value: event.target.value })}
                        placeholder={editing ? "Leave empty to keep the current value" : ""}
                        spellCheck={false}
                        autoFocus={editing !== null}
                    />
                </label>
            </div>
            <div className="vercel-variable-targets" role="group" aria-label="Environments">
                {TARGETS.map((target) => (
                    <Checkbox
                        key={target.value}
                        label={target.label}
                        checked={draft.targets.includes(target.value)}
                        onChange={(on) => toggle(target.value, on)}>
                        {target.label}
                    </Checkbox>
                ))}
                {draft.targets.length === 1 && draft.targets[0] === "preview" ? (
                    <input
                        className="vercel-find vercel-mono"
                        aria-label="Git branch"
                        value={draft.branch}
                        onChange={(event) => setDraft({ ...draft, branch: event.target.value })}
                        placeholder="All preview branches"
                        spellCheck={false}
                    />
                ) : null}
                {editing ? null : (
                    <Checkbox label="Sensitive" checked={draft.sensitive} onChange={(sensitive) => setDraft({ ...draft, sensitive })}>
                        Sensitive
                    </Checkbox>
                )}
            </div>
            <div className="vercel-form-foot">
                <span className="vercel-muted">{problem ?? "The value goes to Vercel and is never shown here or to agents."}</span>
                <button type="button" className="vercel-button" onClick={onDone}>
                    Cancel
                </button>
                <button type="submit" className="vercel-button primary" disabled={problem !== null || saving}>
                    {saving ? "Saving…" : editing ? "Save" : "Add"}
                </button>
            </div>
        </form>
    );
}
