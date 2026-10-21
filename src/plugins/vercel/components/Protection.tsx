import { useState } from "react";
import { confirmDialog, copyText, notify } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { errorText } from "../../../plugin-api/format";
import { vercelApi, type VercelDeployment } from "../api";
import { vercel, vercelProtectionR } from "../resources";

/** Whether the deployment asks visitors to sign in, and a link that lets someone past it. */
export function Protection({ active, team, deployment }: { active: boolean; team: string; deployment: VercelDeployment }) {
    const project = deployment.projectId ?? deployment.project;
    const protection = useResourceEnabled(active, vercelProtectionR, team, project);
    const [busy, setBusy] = useState(false);
    if (!protection.data) return null;
    const { vercelAuthentication, password, canShare } = protection.data;
    const guarded = vercelAuthentication !== null || password !== null;

    const share = async () => {
        if (!deployment.url) return;
        if (!canShare) {
            const sure = await confirmDialog({
                title: "Make a share link?",
                body: `This makes a secret for ${deployment.project}. Anyone holding a link with it can open its protected deployments without signing in, until the secret is removed in Vercel's Deployment Protection settings.`,
                confirmLabel: "Make link",
            });
            if (!sure) return;
        }
        setBusy(true);
        try {
            const { link } = await vercelApi.shareLink(team, project, deployment.url);
            await copyText(link);
            notify("success", "Share link copied");
            vercel.refresh(vercelProtectionR);
        } catch (error) {
            notify("error", `Share link: ${errorText(error)}`);
        } finally {
            setBusy(false);
        }
    };

    return (
        <div className="vercel-protection" aria-label="Deployment Protection">
            <span className="vercel-muted">Protection</span>
            <span>
                {guarded
                    ? [
                          vercelAuthentication && `Vercel Authentication on ${vercelAuthentication.toLowerCase()}`,
                          password && `Password on ${password.toLowerCase()}`,
                      ]
                          .filter(Boolean)
                          .join(" · ")
                    : "None. Anyone with the address can open it."}
            </span>
            {guarded && deployment.url ? (
                <button type="button" className="vercel-link" disabled={busy} onClick={() => void share()}>
                    Copy share link
                </button>
            ) : null}
        </div>
    );
}
