import { focusSignInBrowser, openSignInUrl, swallow } from "../../../plugin-api/host";
import { invalidate, useResource, useResourceEnabled } from "../../../plugin-api/resources";
import { IconAws, SignInScreen, SignInWaiting, useSignInAttempt } from "../../../plugin-api/ui";
import { awsApi, type AwsProfile } from "../api";
import { deriveAuthState } from "../auth";
import { awsIdentityR, awsProfilesR } from "../resources";
import { setAwsProfile } from "../state";

const mark = <IconAws size={28} className="icon-aws" />;

function ProfileList({ profiles }: { profiles: AwsProfile[] }) {
    return (
        <div className="signin-choices">
            {profiles.map((p) => (
                <button key={p.name} type="button" className="signin-choice" onClick={() => setAwsProfile(p.name)}>
                    <IconAws size={14} className="icon-aws" />
                    <span className="signin-choice-name">{p.name}</span>
                    <span className="signin-choice-meta">{[p.kind === "sso" ? "SSO" : p.kind, p.region].filter(Boolean).join(" · ")}</span>
                </button>
            ))}
        </div>
    );
}

export function AwsAuthEmpty({ mode, profile }: { mode: "no-profile" | "unauthed"; profile?: string }) {
    const profilesR = useResource(awsProfilesR);
    const profiles = profilesR.data ?? null;

    if (mode === "no-profile") {
        return (
            <div className="aws-pane">
                <SignInScreen
                    mark={mark}
                    title="Pick an AWS profile"
                    lede={
                        <>
                            Services, logs and billing for one account at a time. Profiles come from <code>~/.aws/config</code>.
                        </>
                    }>
                    {profiles === null && <p className="signin-note">Looking for profiles…</p>}
                    {profiles !== null && profiles.length === 0 && (
                        <div className="signin-callout">
                            No profiles found. Run <code>aws configure sso</code> to add one.
                        </div>
                    )}
                    {profiles && profiles.length > 0 && <ProfileList profiles={profiles} />}
                </SignInScreen>
            </div>
        );
    }

    return <SignedOut profile={profile ?? ""} profiles={profiles} />;
}

function SignedOut({ profile, profiles }: { profile: string; profiles: AwsProfile[] | null }) {
    const identity = useResourceEnabled(!!profile, awsIdentityR, profile, false);
    const auth = deriveAuthState(profile, identity);
    const attempt = useSignInAttempt();
    const waiting = attempt.waiting;

    const signIn = async () => {
        await focusSignInBrowser().catch(swallow("bring the sign-in browser forward"));
        attempt.follow(awsApi.ssoLogin(profile), (result) => {
            if (!result.success) return result.stderr.trim() || "AWS turned the sign-in down";
            void awsApi
                .identity(profile, true)
                .catch(swallow("refresh AWS identity"))
                .then(() => invalidate((kind, args) => kind.startsWith("aws.") && args[0] === profile));
        });
    };

    const ssoUrl = profiles?.find((p) => p.name === profile)?.sso_start_url ?? null;
    const message =
        attempt.error ??
        (auth.kind === "error"
            ? auth.message
            : auth.kind === "no-profile"
              ? ""
              : ((auth as { identity?: { message?: string | null } }).identity?.message ?? ""));
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
        <SignInScreen
            mark={mark}
            title={title}
            lede={
                auth.kind === "cli-missing" ? (
                    <>Sikemux signs in through the AWS CLI. Install it, then retry.</>
                ) : (
                    <>
                        Profile <code>{profile}</code> needs a fresh SSO token to keep browsing.
                    </>
                )
            }
            foot={
                <>
                    Signing in runs <code>aws sso login</code>, so the CLI shares the session.
                </>
            }>
            {waiting ? (
                <SignInWaiting onCancel={attempt.cancel}>Approve the request in your browser</SignInWaiting>
            ) : (
                <button type="button" className="signin-btn primary" disabled={auth.kind === "cli-missing"} onClick={() => void signIn()}>
                    <IconAws size={14} />
                    Continue with AWS SSO
                </button>
            )}

            {message && !waiting && (
                <div className="signin-callout" data-tone="danger">
                    {message.length > 320 ? message.slice(0, 320) + "…" : message}
                </div>
            )}

            <div className="signin-alt">
                <button type="button" className="signin-link" disabled={waiting} onClick={() => void identity.refresh()}>
                    Retry
                </button>
                {ssoUrl && (
                    <button type="button" className="signin-link" onClick={() => void openSignInUrl(ssoUrl).catch(swallow("open the SSO portal"))}>
                        Open the SSO portal
                    </button>
                )}
            </div>

            {others.length > 0 && (
                <>
                    <div className="signin-or">or use another profile</div>
                    <ProfileList profiles={others} />
                </>
            )}
        </SignInScreen>
    );
}
