import { useResource, useResourceEnabled } from "../../../plugin-api/resources";
import { IconAws } from "../../../plugin-api/ui";
import type { AwsProfile } from "../api";
import { deriveAuthState } from "../auth";
import { awsIdentityR, awsProfilesR } from "../resources";
import { openAwsAuthModal, setAwsProfile } from "../state";

function ProfileList({ profiles, current }: { profiles: AwsProfile[]; current?: string }) {
    return (
        <div className="aws-profiles">
            {profiles.map((p) => (
                <button key={p.name} className={`aws-profile${p.name === current ? " sel" : ""}`} onClick={() => setAwsProfile(p.name)}>
                    <span className="aws-account-logo small">
                        <IconAws size={14} />
                    </span>
                    <span className="aws-profile-name">{p.name}</span>
                    <span className="aws-profile-meta">{[p.kind === "sso" ? "SSO" : p.kind, p.region].filter(Boolean).join(" · ")}</span>
                </button>
            ))}
        </div>
    );
}

export function AwsAuthEmpty({ mode, profile }: { mode: "no-profile" | "unauthed"; profile?: string }) {
    const profilesR = useResource(awsProfilesR);
    const identity = useResourceEnabled(!!profile, awsIdentityR, profile ?? "", false);
    const auth = deriveAuthState(profile ?? null, identity);
    const profiles = profilesR.data ?? null;

    if (mode === "no-profile") {
        return (
            <div className="aws-pane">
                <div className="aws-signin">
                    <div className="aws-signin-card">
                        <span className="aws-account-logo large">
                            <IconAws size={26} />
                        </span>
                        <h2>Pick a profile</h2>
                        <p>
                            Profiles come from <code>~/.aws/config</code>. Run <code>aws configure sso</code> first if you don't have any.
                        </p>
                        {profiles === null && <div className="aws-insp-note">Looking for profiles…</div>}
                        {profiles !== null && profiles.length === 0 && (
                            <div className="aws-insp-note">
                                No profiles found. Run <code>aws configure sso</code> to add one.
                            </div>
                        )}
                        {profiles && profiles.length > 0 && <ProfileList profiles={profiles} />}
                    </div>
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
    const others = (profiles ?? []).filter((p) => p.name !== profile);
    return (
        <div className="aws-signin">
            <div className="aws-signin-card">
                <span className="aws-account-logo large">
                    <IconAws size={26} />
                </span>
                <h2>{title}</h2>
                <p>
                    Profile <code>{profile}</code> needs a fresh SSO token. Sign in again to keep browsing.
                </p>
                {message && <pre className="aws-signin-err">{message.length > 320 ? message.slice(0, 320) + "…" : message}</pre>}
                <div className="aws-signin-actions">
                    <button className="aws-btn primary" onClick={() => openAwsAuthModal(profile ?? "", ssoUrl)}>
                        Sign in with SSO
                    </button>
                    <button className="aws-btn" onClick={() => void identity.refresh()}>
                        Retry
                    </button>
                </div>
                {others.length > 0 && (
                    <>
                        <div className="aws-signin-or">Or use another profile</div>
                        <ProfileList profiles={others} />
                    </>
                )}
            </div>
        </div>
    );
}
