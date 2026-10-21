import { useState } from "react";
import { confirmDialog, notify, promptDialog } from "../../../plugin-api/host";
import { errorText } from "../../../plugin-api/format";
import { kubeApi, type KubeAction, type Scope, type Workload } from "../api";
import { kubernetes } from "../resources";
import { kubectlScope } from "./PodView";

export interface WorkloadAction {
    action: KubeAction;
    label: string;
}

/** What can be done to a workload of each kind, as kubectl allows. */
export function actionsFor(workload: Pick<Workload, "kind" | "suspended">): WorkloadAction[] {
    switch (workload.kind) {
        case "Deployment":
        case "StatefulSet":
            return [
                { action: "restart", label: "Restart" },
                { action: "scale", label: "Scale…" },
            ];
        case "DaemonSet":
            return [{ action: "restart", label: "Restart" }];
        case "CronJob":
            return [
                { action: "runNow", label: "Run now" },
                workload.suspended ? { action: "resume", label: "Resume" } : { action: "suspend", label: "Pause" },
            ];
        default:
            return [];
    }
}

/** The replica count typed into the scale prompt, or why it can't be used. */
export function replicasOf(typed: string): number | string {
    const replicas = Number(typed.trim());
    if (!/^\d+$/.test(typed.trim())) return "Type a whole number of replicas.";
    if (replicas > 50) return "Sikemux scales to at most 50. Use kubectl for more.";
    return replicas;
}

async function confirmed(action: KubeAction, workload: Workload, scope: Scope): Promise<number | null | false> {
    const target = `${workload.kind.toLowerCase()}/${workload.name}`;
    const kubectl = ["kubectl", ...kubectlScope(scope)].join(" ");
    if (action === "scale") {
        const typed = await promptDialog({
            title: `Scale ${workload.name}`,
            body: `It runs ${workload.desired ?? 0} now.`,
            label: "Replicas",
            initial: String(workload.desired ?? 1),
            confirmLabel: "Scale",
        });
        if (typed === null) return false;
        const replicas = replicasOf(typed);
        if (typeof replicas === "string") {
            notify("error", replicas);
            return false;
        }
        if (replicas === 0) {
            const stop = await confirmDialog({
                title: `Stop every pod of ${workload.name}?`,
                body: "Scaling to 0 leaves nothing serving until it is scaled up again.",
                confirmLabel: "Scale to 0",
                destructive: true,
                commands: [{ label: "Runs", command: `${kubectl} scale ${target} --replicas=0` }],
            });
            return stop ? 0 : false;
        }
        return replicas;
    }
    const asked = {
        restart: {
            title: `Restart ${workload.name}?`,
            body: "Its pods are replaced one after another, so it keeps serving if it runs more than one.",
            command: `${kubectl} rollout restart ${target}`,
        },
        runNow: {
            title: `Run ${workload.name} now?`,
            body: "Starts its job once, outside the schedule.",
            command: `${kubectl} create job ${workload.name}-manual-… --from=${target}`,
        },
        suspend: {
            title: `Pause ${workload.name}?`,
            body: "No new jobs start until it is resumed. A job already running carries on.",
            command: `${kubectl} patch ${target} -p '{"spec":{"suspend":true}}'`,
        },
        resume: {
            title: `Resume ${workload.name}?`,
            body: "Its jobs start again on schedule.",
            command: `${kubectl} patch ${target} -p '{"spec":{"suspend":false}}'`,
        },
        deletePod: null,
    }[action];
    if (!asked) return false;
    const yes = await confirmDialog({
        title: asked.title,
        body: asked.body,
        confirmLabel: actionsFor(workload).find((each) => each.action === action)?.label ?? "Go",
        commands: [{ label: "Runs", command: asked.command }],
    });
    return yes ? null : false;
}

export function WorkloadActions({ workload, scope }: { workload: Workload; scope: Scope }) {
    const [busy, setBusy] = useState<KubeAction | null>(null);
    const actions = actionsFor(workload);
    if (actions.length === 0) return null;

    const run = async (action: KubeAction) => {
        const answer = await confirmed(action, workload, scope);
        if (answer === false) return;
        setBusy(action);
        try {
            const done = await kubeApi.act(scope, action, workload.name, workload.kind, answer ?? undefined);
            notify("success", done.message || `${workload.name}: done`);
            kubernetes.refresh();
        } catch (failure) {
            notify("error", errorText(failure));
        } finally {
            setBusy(null);
        }
    };

    return (
        <span className="k8s-row-actions">
            {actions.map((each) => (
                <button
                    key={each.action}
                    type="button"
                    className="k8s-button small"
                    disabled={busy !== null}
                    aria-label={`${each.label.replace("…", "")} ${workload.name}`}
                    onClick={(event) => {
                        event.stopPropagation();
                        void run(each.action);
                    }}
                    onKeyDown={(event) => event.stopPropagation()}>
                    {busy === each.action ? "…" : each.label}
                </button>
            ))}
        </span>
    );
}
