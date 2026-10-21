import { useState } from "react";
import { confirmDialog, notify } from "../../../plugin-api/host";
import { errorText, formatAgo } from "../../../plugin-api/format";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, Pending } from "../../../plugin-api/ui";
import { vercelApi, type VercelVariable } from "../api";
import { vercel, vercelEnvR } from "../resources";
import { VariableForm } from "./VariableForm";

const ALL_TARGETS = ["production", "preview", "development"];

const targetsLabel = (targets: string[]) =>
    ALL_TARGETS.every((target) => targets.includes(target))
        ? "All Environments"
        : targets.map((target) => target.charAt(0).toUpperCase() + target.slice(1)).join(", ");

async function removeVariable(team: string, project: string, variable: VercelVariable): Promise<void> {
    const sure = await confirmDialog({
        title: `Remove ${variable.key}?`,
        body: `Deployments of ${targetsLabel(variable.target)} built from now on won't have it. The ones running now keep it.`,
        confirmLabel: "Remove",
        destructive: true,
    });
    if (!sure) return;
    try {
        await vercelApi.removeEnv(team, project, variable.id);
        vercel.refresh(vercelEnvR);
        notify("success", `Removed ${variable.key}`);
    } catch (error) {
        notify("error", `Remove ${variable.key}: ${errorText(error)}`);
    }
}

/** A project's environment variables as Vercel lists them, by name only, with adding, changing and removing them. */
export function Environment({ active, team, project, find }: { active: boolean; team: string; project: string; find: string }) {
    const variables = useResourceEnabled(active, vercelEnvR, team, project);
    const [form, setForm] = useState<"new" | VercelVariable | null>(null);
    if (!variables.data) return <Pending handle={variables} loading="Loading environment variables" rows={5} />;
    const words = find.trim().toLowerCase();
    const shown = variables.data.filter((variable) => !words || variable.key.toLowerCase().includes(words));
    return (
        <div className="vercel-settings">
            <div className="vercel-form-foot">
                <p className="vercel-note vercel-muted">Names only. Values stay in Vercel and are never shown here or given to agents.</p>
                {form === null ? (
                    <button type="button" className="vercel-button primary" onClick={() => setForm("new")}>
                        Add New
                    </button>
                ) : null}
            </div>
            {form !== null ? (
                <VariableForm
                    key={form === "new" ? "new" : form.id}
                    team={team}
                    project={project}
                    editing={form === "new" ? null : form}
                    onDone={() => setForm(null)}
                />
            ) : null}
            {shown.length === 0 ? (
                <EmptyState message={variables.data.length === 0 ? "No Environment Variables Added" : "No variable matches."} />
            ) : (
                <div className="vercel-table" role="table" aria-label="Environment variables">
                    {shown.map((variable) => (
                        <div key={variable.id} className="vercel-table-row env" role="row" aria-label={variable.key}>
                            <span className="vercel-cell vercel-mono key">{variable.key}</span>
                            <span className="vercel-cell">
                                {targetsLabel(variable.target)}
                                {variable.gitBranch && <span className="vercel-muted"> · {variable.gitBranch}</span>}
                            </span>
                            <span className="vercel-cell">
                                <span className="vercel-env">{variable.type}</span>
                            </span>
                            <span className="vercel-cell when vercel-muted">
                                {variable.updatedAt ? `Updated ${formatAgo(variable.updatedAt)}` : ""}
                            </span>
                            <span className="vercel-cell vercel-row-actions">
                                <button type="button" className="vercel-link" onClick={() => setForm(variable)}>
                                    Edit
                                </button>
                                <button type="button" className="vercel-link danger" onClick={() => void removeVariable(team, project, variable)}>
                                    Remove
                                </button>
                            </span>
                        </div>
                    ))}
                </div>
            )}
        </div>
    );
}
