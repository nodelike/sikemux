import { useState, type ReactNode } from "react";
import { confirmDialog, copyWithNotice, notify, openUrl, swallow } from "../../../plugin-api/host";
import { errorText, formatAgo } from "../../../plugin-api/format";
import type { ContextMenuItem } from "../../../plugin-api/ui";
import { vercelApi, type VercelDeployment } from "../api";

const STATE_LABELS: Record<string, string> = {
    READY: "Ready",
    ERROR: "Error",
    BUILDING: "Building",
    QUEUED: "Queued",
    INITIALIZING: "Initializing",
    CANCELED: "Canceled",
};

export const stateLabel = (state: string) => STATE_LABELS[state] ?? state;

export const visit = (url: string) => void openUrl(url).catch(swallow("open the page"));

export const shortId = (id: string) => id.replace(/^dpl_/, "").slice(0, 9);

export const hostOf = (url: string | null) => url?.replace(/^https?:\/\//, "") ?? null;

export function StateDot({ state }: { state: string }) {
    return <span className={`vercel-dot ${state.toLowerCase()}`} aria-hidden="true" />;
}

export function Status({ deployment, duration }: { deployment: Pick<VercelDeployment, "state" | "buildMs">; duration: (ms: number) => string }) {
    return (
        <span className="vercel-status">
            <StateDot state={deployment.state} />
            <span>{stateLabel(deployment.state)}</span>
            {deployment.buildMs !== null && <span className="vercel-muted">{duration(deployment.buildMs)}</span>}
        </span>
    );
}

function Svg({ children, size = 14 }: { children: ReactNode; size?: number }) {
    return (
        <svg
            width={size}
            height={size}
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.4"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true">
            {children}
        </svg>
    );
}

export const IconPromote = () => (
    <Svg>
        <circle cx="8" cy="8" r="6.2" />
        <path d="M8 11V5M5.5 7.5 8 5l2.5 2.5" />
    </Svg>
);

export const IconRollback = () => (
    <Svg>
        <path d="M3.5 6.5A5 5 0 1 1 3 9" />
        <path d="M3.2 3.2v3.3h3.3" />
    </Svg>
);

export const IconCommit = () => (
    <Svg>
        <circle cx="8" cy="8" r="2.4" />
        <path d="M1.5 8h4.1M10.4 8h4.1" />
    </Svg>
);

export const IconBranch = () => (
    <Svg>
        <circle cx="4.5" cy="3.5" r="1.6" />
        <circle cx="4.5" cy="12.5" r="1.6" />
        <circle cx="11.5" cy="5" r="1.6" />
        <path d="M4.5 5.1v5.8M11.5 6.6c0 3-7 2.4-7 4.3" />
    </Svg>
);

export const IconExternal = () => (
    <Svg size={12}>
        <path d="M9.5 2.5h4v4M13.5 2.5 7.5 8.5M12 9.5v3.5a.5.5 0 0 1-.5.5h-8a.5.5 0 0 1-.5-.5v-8a.5.5 0 0 1 .5-.5H7" />
    </Svg>
);

export const IconMore = () => (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
        <circle cx="3.5" cy="8" r="1.3" />
        <circle cx="8" cy="8" r="1.3" />
        <circle cx="12.5" cy="8" r="1.3" />
    </svg>
);

export const IconGitHub = () => (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
        <path d="M8 .2a8 8 0 0 0-2.5 15.6c.4 0 .5-.2.5-.4v-1.5c-2.2.5-2.7-1-2.7-1-.4-.9-.9-1.2-.9-1.2-.7-.5.1-.5.1-.5.8.1 1.2.8 1.2.8.7 1.3 1.9.9 2.3.7.1-.5.3-.9.5-1.1-1.8-.2-3.6-.9-3.6-4 0-.9.3-1.6.8-2.1-.1-.2-.4-1 .1-2.1 0 0 .7-.2 2.2.8a7.6 7.6 0 0 1 4 0c1.5-1 2.2-.8 2.2-.8.4 1.1.2 1.9.1 2.1.5.6.8 1.3.8 2.1 0 3.1-1.9 3.7-3.6 3.9.3.3.6.8.6 1.5v2.2c0 .2.1.5.6.4A8 8 0 0 0 8 .2Z" />
    </svg>
);

/** Production or Preview, filled for the deployment serving production now. */
export function EnvBadge({ deployment, current }: { deployment: Pick<VercelDeployment, "target">; current: boolean }) {
    return (
        <span className={`vercel-env${current ? " current" : ""}`} title={current ? "Serving production now" : undefined}>
            {deployment.target === "production" && <IconPromote />}
            {deployment.target === "production" ? "Production" : "Preview"}
        </span>
    );
}

/** Where a deployment came from: a redeploy of another, or a commit on a branch. */
export function Source({ deployment }: { deployment: VercelDeployment }) {
    if (deployment.redeployOf) {
        return (
            <span className="vercel-source">
                <span className="vercel-source-part">
                    <IconRollback />
                    <span>Redeploy of</span>
                    <span className="vercel-mono">{shortId(deployment.redeployOf)}</span>
                </span>
            </span>
        );
    }
    return (
        <span className="vercel-source">
            {deployment.commit && (
                <span className="vercel-source-part">
                    <IconCommit />
                    <span className="vercel-mono">{deployment.commit.slice(0, 7)}</span>
                </span>
            )}
            {deployment.branch && (
                <span className="vercel-source-part">
                    <IconBranch />
                    <span className="vercel-mono">{deployment.branch}</span>
                </span>
            )}
        </span>
    );
}

/** The person's initial in a circle of their own colour; the app does not load pictures from other sites. */
export function Avatar({ name }: { name: string | null }) {
    const label = name?.trim() || "?";
    let hue = 0;
    for (const char of label) hue = (hue * 31 + char.charCodeAt(0)) % 360;
    return (
        <span className="vercel-avatar" style={{ background: `hsl(${hue} 55% 45%)` }} title={label} aria-hidden="true">
            {label.charAt(0).toUpperCase()}
        </span>
    );
}

export const Ago = ({ at }: { at: number }) => <span className="vercel-muted">{formatAgo(at)}</span>;

/** What can be done to a deployment, each asking the person first, as Vercel's own menu offers them. */
export function useDeploymentActions(team: string) {
    const [busy, setBusy] = useState(false);
    const act = async (confirm: { title: string; body: string; confirmLabel: string }, done: string, run: () => Promise<unknown>) => {
        if (busy || !(await confirmDialog(confirm))) return;
        setBusy(true);
        try {
            await run();
            notify("success", done);
        } catch (error) {
            notify("error", `${confirm.confirmLabel}: ${errorText(error)}`);
        } finally {
            setBusy(false);
        }
    };
    const name = (deployment: VercelDeployment) => deployment.message || hostOf(deployment.url) || deployment.id;

    const rollback = (deployment: VercelDeployment) =>
        act(
            {
                title: "Roll back production?",
                body: `${name(deployment)} (${formatAgo(deployment.createdAt)}) starts serving production again, without building.`,
                confirmLabel: "Roll back",
            },
            "Rolled production back",
            () => vercelApi.promote(team, deployment.id),
        );
    const promote = (deployment: VercelDeployment) =>
        act(
            {
                title: "Promote to production?",
                body: `Vercel builds ${name(deployment)} for production, and it serves the production domains once it is ready.`,
                confirmLabel: "Promote",
            },
            "Promoting to production",
            () => vercelApi.redeploy(team, deployment.id, "production"),
        );
    const redeploy = (deployment: VercelDeployment) =>
        act(
            {
                title: "Redeploy?",
                body: `Vercel builds ${name(deployment)} again as a new ${deployment.target} deployment.`,
                confirmLabel: "Redeploy",
            },
            "Redeploying",
            () => vercelApi.redeploy(team, deployment.id),
        );
    const cancel = (deployment: VercelDeployment) =>
        act(
            { title: "Cancel this deployment?", body: `${name(deployment)} stops building.`, confirmLabel: "Cancel deployment" },
            "Canceled the deployment",
            () => vercelApi.cancel(team, deployment.id),
        );

    const menu = (deployment: VercelDeployment, current: boolean, extra: ContextMenuItem[] = []): ContextMenuItem[] => {
        const ready = deployment.state === "READY";
        const underway = deployment.state === "BUILDING" || deployment.state === "QUEUED" || deployment.state === "INITIALIZING";
        return [
            {
                label: "Instant Rollback",
                icon: <IconRollback />,
                disabled: busy || current || !ready || deployment.target !== "production",
                run: () => void rollback(deployment),
            },
            {
                label: "Promote",
                icon: <IconPromote />,
                disabled: busy || !ready || deployment.target === "production",
                run: () => void promote(deployment),
            },
            { sep: true },
            underway
                ? { label: "Cancel Deployment", disabled: busy, run: () => void cancel(deployment) }
                : { label: "Redeploy", disabled: busy, run: () => void redeploy(deployment) },
            { label: "Inspect Deployment", disabled: !deployment.inspectorUrl, run: () => deployment.inspectorUrl && visit(deployment.inspectorUrl) },
            {
                label: "View Source",
                disabled: !deployment.inspectorUrl,
                run: () => deployment.inspectorUrl && visit(`${deployment.inspectorUrl}/source`),
            },
            { label: "Copy URL", disabled: !deployment.url, run: () => deployment.url && copyWithNotice(deployment.url, "the URL") },
            { label: "Visit", icon: <IconExternal />, disabled: !deployment.url || !ready, run: () => deployment.url && visit(deployment.url) },
            ...extra,
        ];
    };

    return { busy, rollback, promote, redeploy, cancel, menu };
}
