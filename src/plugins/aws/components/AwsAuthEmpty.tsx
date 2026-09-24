import { useResource, useResourceEnabled } from "../../../plugin-api/resources";
import { deriveAuthState } from "../auth";
import { awsIdentityR, awsProfilesR } from "../resources";
import { openAwsAuthModal, setAwsProfile } from "../state";

export function AwsAuthEmpty({ mode, profile }: { mode: "no-profile" | "unauthed"; profile?: string }) {
    const profilesR = useResource(awsProfilesR);
    const identity = useResourceEnabled(!!profile, awsIdentityR, profile ?? "", false);
    const auth = deriveAuthState(profile ?? null, identity);
    const profiles = profilesR.data ?? null;

    if (mode === "no-profile") {
        return (
            <div className="aws-empty-stage">
                <div className="aws-empty-card">
                    <div className="aws-empty-label">AWS</div>
                    <div className="aws-empty-title">Pick a profile</div>
                    <div className="aws-empty-sub">
                        Profiles come from <code>~/.aws/config</code>. Run <code>aws configure sso</code> first if you don't have any.
                    </div>
                    {profiles === null && <div className="aws-empty-loading">scanning…</div>}
                    {profiles !== null && profiles.length === 0 && (
                        <div className="aws-empty-loading">
                            no profiles found — run <code>aws configure sso</code> to add one
                        </div>
                    )}
                    {profiles && profiles.length > 0 && (
                        <div className="aws-profile-list">
                            {profiles.map((p) => (
                                <button
                                    key={p.name}
                                    className="aws-profile-row"
                                    onClick={() => {
                                        setAwsProfile(p.name);
                                    }}>
                                    <span className="aws-profile-name">{p.name}</span>
                                    <span className="aws-profile-meta">
                                        {p.kind === "sso" ? "SSO" : p.kind}
                                        {p.region ? ` · ${p.region}` : ""}
                                    </span>
                                </button>
                            ))}
                        </div>
                    )}
                </div>
            </div>
        );
    }

    const ssoUrl = profiles?.find((p) => p.name === profile)?.sso_start_url ?? null;
    const message =
        auth.kind === "error"
            ? auth.message
            : auth.kind === "no-profile"
              ? ""
              : ((auth as { identity?: { message?: string | null } }).identity?.message ?? "");
    const title =
        auth.kind === "expired"
            ? "Session expired"
            : auth.kind === "no-credentials"
              ? "No credentials"
              : auth.kind === "cli-missing"
                ? "AWS CLI missing"
                : "Not signed in";
    return (
        <div className="aws-empty-stage">
            <div className="aws-empty-card">
                <div className="aws-empty-label">AWS</div>
                <div className="aws-empty-title">{title}</div>
                <div className="aws-empty-sub">
                    Profile <code>{profile}</code> needs a fresh SSO token.
                </div>
                {message && <pre className="aws-empty-err">{message.length > 320 ? message.slice(0, 320) + "…" : message}</pre>}
                <div className="aws-empty-actions">
                    <button className="aws-empty-btn primary" onClick={() => openAwsAuthModal(profile ?? "", ssoUrl)}>
                        Sign in with SSO
                    </button>
                    <button className="aws-empty-btn" onClick={() => void identity.refresh()}>
                        Retry
                    </button>
                    <button className="aws-empty-btn ghost" onClick={() => setAwsProfile(null)}>
                        Switch profile
                    </button>
                </div>
            </div>
        </div>
    );
}
