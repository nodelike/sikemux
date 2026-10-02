import { useEffect, useId, useRef, useState } from "react";
import { notify, reportError, useModalFocus, usePluginOverlay } from "../../plugin-api/host";
import { invalidate, useResource } from "../../plugin-api/resources";
import { IconClose, IconPlus, IconRun } from "../../plugin-api/ui";
import { hostApi, type RepoRef, type Workflow } from "../api";
import { hostBranchesR } from "../resources";
import { useBusy } from "./hooks";

interface Input {
    key: string;
    name: string;
    value: string;
}

let nextKey = 0;

interface Props {
    repo: RepoRef;
    workflow: Workflow;
    defaultBranch: string | null;
    onClose: () => void;
}

/**
 * Starting a workflow by hand. What inputs it takes is written in its own
 * file, which this does not read, so they are typed as names and values and
 * GitHub says if one is wrong.
 */
export function DispatchDialog({ repo, workflow, defaultBranch, onClose }: Props) {
    const branches = useResource(hostBranchesR, repo);
    const [gitRef, setGitRef] = useState(defaultBranch ?? "");
    const [inputs, setInputs] = useState<Input[]>([]);
    const [busy, runBusy] = useBusy();
    const modalRef = useRef<HTMLDivElement>(null);
    const refId = useId();
    const branchListId = useId();
    usePluginOverlay(true);
    useModalFocus(modalRef);

    // Caught before the panes behind the scrim can act on it.
    useEffect(() => {
        const onKey = (event: KeyboardEvent) => {
            if (event.key !== "Escape") return;
            event.preventDefault();
            event.stopPropagation();
            onClose();
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, [onClose]);

    const submit = async () => {
        if (!gitRef.trim()) return;
        const values: Record<string, string> = {};
        for (const input of inputs) {
            const name = input.name.trim();
            if (name) values[name] = input.value;
        }
        try {
            await hostApi(repo.provider).dispatch(repo, workflow.id, gitRef.trim(), values);
            notify("success", `Started ${workflow.name} on ${gitRef.trim()}`);
            // GitHub takes a moment to register the run, so the list is re-read
            // once rather than immediately showing nothing new.
            setTimeout(() => invalidate((kind) => kind === "host.runs"), 1500);
            onClose();
        } catch (error) {
            reportError(`Could not start ${workflow.name}`)(error);
        }
    };

    return (
        <div className="dlg-scrim" onMouseDown={onClose}>
            <div
                ref={modalRef}
                tabIndex={-1}
                className="dlg"
                role="dialog"
                aria-modal="true"
                aria-label={`Run ${workflow.name}`}
                onMouseDown={(event) => event.stopPropagation()}>
                <div className="dlg-head">
                    <span className="dlg-glyph" aria-hidden="true">
                        <IconRun size={15} />
                    </span>
                    <h2 className="dlg-title">Run {workflow.name}</h2>
                </div>

                <form
                    className="dlg-field"
                    onSubmit={(event) => {
                        event.preventDefault();
                        runBusy(submit);
                    }}>
                    <label htmlFor={refId}>Branch or tag</label>
                    <input
                        id={refId}
                        className="dlg-input gha-mono"
                        list={branchListId}
                        value={gitRef}
                        onChange={(event) => setGitRef(event.target.value)}
                        placeholder="main"
                        autoFocus
                        spellCheck={false}
                    />
                    <datalist id={branchListId}>
                        {(branches.data ?? []).map((branch) => (
                            <option key={branch} value={branch} />
                        ))}
                    </datalist>
                </form>

                <div className="dlg-field gha-inputs">
                    {inputs.map((input, index) => (
                        <div className="gha-input-row" key={input.key}>
                            <input
                                className="dlg-input gha-mono"
                                value={input.name}
                                placeholder="input"
                                aria-label="Input name"
                                spellCheck={false}
                                onChange={(event) =>
                                    setInputs((all) => all.map((each, at) => (at === index ? { ...each, name: event.target.value } : each)))
                                }
                            />
                            <input
                                className="dlg-input gha-mono"
                                value={input.value}
                                placeholder="value"
                                aria-label={`Value of ${input.name || "input"}`}
                                spellCheck={false}
                                onChange={(event) =>
                                    setInputs((all) => all.map((each, at) => (at === index ? { ...each, value: event.target.value } : each)))
                                }
                            />
                            <button
                                type="button"
                                className="gha-icon-btn"
                                aria-label={`Remove ${input.name || "input"}`}
                                onClick={() => setInputs((all) => all.filter((_, at) => at !== index))}>
                                <IconClose size={11} />
                            </button>
                        </div>
                    ))}
                    <button
                        type="button"
                        className="gha-link"
                        onClick={() => setInputs((all) => [...all, { key: String(nextKey++), name: "", value: "" }])}>
                        <IconPlus size={11} /> Add an input
                    </button>
                </div>

                <div className="dlg-foot">
                    <button type="button" className="dlg-btn" onClick={onClose}>
                        Cancel
                    </button>
                    <button type="button" className="dlg-btn primary" disabled={busy || !gitRef.trim()} onClick={() => runBusy(submit)}>
                        {busy ? "Starting…" : "Run workflow"}
                    </button>
                </div>
            </div>
        </div>
    );
}
