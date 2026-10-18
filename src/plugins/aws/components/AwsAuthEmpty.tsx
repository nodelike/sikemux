import { focusSignInBrowser, openSignInUrl, openUrl, runInTerminal, swallow } from "../../../plugin-api/host";
import { invalidate, useResource, useResourceEnabled } from "../../../plugin-api/resources";
import { IconAws, SignInScreen, SignInWaiting, useSignInAttempt } from "../../../plugin-api/ui";
import { awsApi, type AwsProfile } from "../api";
import { deriveAuthState } from "../auth";
import { awsIdentityR, awsProfilesR, awsSetupR } from "../resources";
import { setAwsProfile } from "../state";

const mark = <IconAws size={28} />;
const INSTALL_GUIDE = "https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html";

function checkAgain(): void {
    invalidate((kind) => kind.startsWith("aws."));
}

/** The pane reads everything again once the terminal closes, so it moves on by itself. */
function runSetup(title: string, argv: string[]): void {
    void runInTerminal(title, argv).then(checkAgain);
}

function ProfileList({ profiles }: { profiles: AwsProfile[] }) {
    return (
        <div className="signin-choices">
            {profiles.map((p) => (
                <button key={p.name} type="button" className="signin-choice" onClick={() => setAwsProfile(p.name)}>
                    <IconAws size={14} />
                    <span className="signin-choice-name">{p.name}</span>
                    <span className="signin-choice-meta">{[p.kind === "sso" ? "SSO" : p.kind, p.region].filter(Boolean).join(" · ")}</span>
                </button>
            ))}
        </div>
    );
}

function CheckAgain() {
    return (
        <div className="signin-alt">
            <button type="button" className="signin-link" onClick={checkAgain}>
                Check again
            </button>
        </div>
    );
}

function InstallCli() {
    const homebrew = useResource(awsSetupR).data?.homebrew ?? false;
    return (
        <SignInScreen
            mark={mark}
            title="Install the AWS CLI"
            lede={<>Sikemux and its agents reach AWS through the AWS CLI, which this computer does not have yet.</>}
            foot={
                homebrew ? (
                    <>
                        Runs <code>brew install awscli</code> in a terminal.
                    </>
                ) : (
                    <>Opens Amazon’s install guide.</>
                )
            }>
            {homebrew ? (
                <button type="button" className="signin-btn primary" onClick={() => runSetup("Install the AWS CLI", ["brew", "install", "awscli"])}>
                    Install with Homebrew
                </button>
            ) : (
                <button
                    type="button"
                    className="signin-btn primary"
                    onClick={() => void openUrl(INSTALL_GUIDE).catch(swallow("open the AWS CLI guide"))}>
                    Get the installer
                </button>
            )}
            <CheckAgain />
        </SignInScreen>
    );
}

function SetUpProfile() {
    return (
        <SignInScreen
            mark={mark}
            title="Set up an AWS profile"
            lede={
                <>
                    Sikemux and its agents use the profiles in <code>~/.aws/config</code>, and there are none yet.
                </>
            }
            foot={
                <>
                    Runs <code>aws configure sso</code> in a terminal.
                </>
            }>
            <button type="button" className="signin-btn primary" onClick={() => runSetup("Set up an AWS profile", ["aws", "configure", "sso"])}>
                <IconAws size={14} mono />
                Set up a profile
            </button>
            <div className="signin-alt">
                <button type="button" className="signin-link" onClick={() => runSetup("Set up AWS access keys", ["aws", "configure"])}>
                    Use access keys instead
                </button>
                <button type="button" className="signin-link" onClick={checkAgain}>
                    Check again
                </button>
            </div>
        </SignInScreen>
    );
}

export function AwsAuthEmpty({ mode, profile }: { mode: "no-profile" | "unauthed"; profile?: string }) {
    const profilesR = useResource(awsProfilesR);
    const setup = useResource(awsSetupR).data ?? null;
    const profiles = profilesR.data ?? null;

    if (mode === "unauthed") return <SignedOut profile={profile ?? ""} profiles={profiles} />;
    return (
        <div className="aws-pane">
            {setup && !setup.cli ? (
                <InstallCli />
            ) : profiles !== null && profiles.length === 0 ? (
                <SetUpProfile />
            ) : (
                <SignInScreen
                    mark={mark}
                    title="Pick an AWS profile"
                    lede={
                        <>
                            Services, logs and billing for one account at a time. Profiles come from <code>~/.aws/config</code>.
                        </>
                    }>
                    {profiles === null ? <p className="signin-note">Looking for profiles…</p> : <ProfileList profiles={profiles} />}
                </SignInScreen>
            )}
        </div>
    );
}

function SignedOut({ profile, profiles }: { profile: string; profiles: AwsProfile[] | null }) {
    const identity = useResourceEnabled(!!profile, awsIdentityR, profile, false);
    const auth = deriveAuthState(profile, identity);
    const attempt = useSignInAttempt();
    const waiting = attempt.waiting;

    if (auth.kind === "cli-missing") return <InstallCli />;

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

    const chosen = profiles?.find((p) => p.name === profile);
    const usesSso = chosen?.kind !== "credentials" || auth.kind === "expired";
    const ssoUrl = chosen?.sso_start_url ?? null;
    const message =
        attempt.error ??
        (auth.kind === "error"
            ? auth.message
            : auth.kind === "no-profile"
              ? ""
              : ((auth as { identity?: { message?: string | null } }).identity?.message ?? ""));
    const title = auth.kind === "expired" ? "Sign-in expired" : auth.kind === "no-credentials" ? "No credentials" : "Not signed in";
    const others = (profiles ?? []).filter((p) => p.name !== profile);

    return (
        <SignInScreen
            mark={mark}
            title={title}
            lede={
                usesSso ? (
                    <>
                        Profile <code>{profile}</code> needs a fresh sign-in. Agents use it too, and only you can sign in.
                    </>
                ) : (
                    <>
                        Profile <code>{profile}</code> has no working access keys.
                    </>
                )
            }
            foot={
                usesSso ? (
                    <>
                        Signing in runs <code>aws sso login</code>, so the CLI shares the session.
                    </>
                ) : (
                    <>
                        Runs <code>aws configure</code> for this profile in a terminal.
                    </>
                )
            }>
            {waiting ? (
                <SignInWaiting onCancel={attempt.cancel}>Approve the request in your browser</SignInWaiting>
            ) : usesSso ? (
                <button type="button" className="signin-btn primary" onClick={() => void signIn()}>
                    <IconAws size={14} mono />
                    Sign in
                </button>
            ) : (
                <button
                    type="button"
                    className="signin-btn primary"
                    onClick={() => runSetup("Set up AWS access keys", ["aws", "configure", "--profile", profile])}>
                    Set up access keys
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
