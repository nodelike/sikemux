import { useState } from "react";
import { openUrl, swallow } from "../../../plugin-api/host";
import { resource, useResource } from "../../../plugin-api/resources";
import { SignInError, SignInScreen, SkeletonRows, useSignInAttempt } from "../../../plugin-api/ui";
import { actionsApi, type ActionsStatus } from "../api";
import { GithubMark } from "./GithubMark";

/** The sign-in form wants to know where a borrowed token came from, which only GitHub's own status says. */
const githubStatusR = resource({
    kind: "host.githubStatus",
    fetch: (): Promise<ActionsStatus> => actionsApi.status(),
    staleAfterMs: 60_000,
});

export function GithubSignIn({ onSignedIn }: { onSignedIn: (account: string | null) => void }) {
    const status = useResource(githubStatusR);
    if (!status.data) return <SkeletonRows rows={4} label="Connecting to GitHub" />;
    return <ActionsSignIn status={status.data} onSignedIn={onSignedIn} />;
}

function sourceNote(status: ActionsStatus): string {
    if (status.tokenSource === "ghCli") return "The gh CLI is signed in here, so Sikemux uses its token.";
    return `${status.tokenVariable ?? "A token variable"} is set in your shell, so Sikemux uses that token.`;
}

function tokenPage(host: string): string {
    return `https://${host}/settings/tokens/new?scopes=repo,workflow&description=Sikemux`;
}

interface Props {
    status: ActionsStatus;
    onSignedIn: (account: string | null) => void;
}

/**
 * The token is the only thing to ask for, and often there is not even that:
 * a shell that exports one, or a signed-in `gh`, is offered as it is.
 */
export function ActionsSignIn({ status, onSignedIn }: Props) {
    const [host, setHost] = useState(status.host);
    const [editingHost, setEditingHost] = useState(status.host !== "github.com");
    const [token, setToken] = useState("");
    const attempt = useSignInAttempt(status);
    const busy = attempt.busy;

    const borrowed = status.tokenSource === "environment" || status.tokenSource === "ghCli";
    const canSubmit = !busy && !!token.trim();

    const submit = (withToken: string | undefined) =>
        attempt.run(async () => {
            const result = await actionsApi.signIn(host.trim(), withToken);
            if (!result.ok) return result.message ?? "GitHub turned that token down";
            onSignedIn(result.account);
        });

    return (
        <SignInScreen
            mark={<GithubMark size={26} />}
            title="Connect GitHub"
            lede="Pull requests, Actions runs and issues for this repository, right beside your changes."
            foot="Sikemux keeps your sign-in in the macOS Keychain.">
            {borrowed && (
                <>
                    <button type="button" className="signin-btn primary" disabled={busy} onClick={() => submit(undefined)}>
                        <GithubMark size={14} />
                        {busy
                            ? "Checking…"
                            : status.tokenSource === "ghCli"
                              ? "Continue with the gh CLI"
                              : `Continue with ${status.tokenVariable ?? "that token"}`}
                    </button>
                    <p className="signin-note">{sourceNote(status)}</p>
                    <div className="signin-or">or paste a token</div>
                </>
            )}

            <div className="signin-form">
                {editingHost && (
                    <label className="signin-field">
                        <span>Host</span>
                        <input
                            className="signin-input mono"
                            value={host}
                            onChange={(event) => setHost(event.target.value)}
                            placeholder="github.com"
                            spellCheck={false}
                            autoCapitalize="off"
                            autoCorrect="off"
                        />
                    </label>
                )}
                <label className="signin-field">
                    <span>Personal access token</span>
                    <input
                        className="signin-input mono"
                        type="password"
                        value={token}
                        onChange={(event) => setToken(event.target.value)}
                        onKeyDown={(event) => {
                            if (event.key === "Enter" && canSubmit) submit(token.trim());
                        }}
                        placeholder="ghp_… or github_pat_…"
                        autoFocus={!borrowed}
                        spellCheck={false}
                    />
                    <small className="signin-hint">
                        It needs <code>repo</code> to read private repositories and <code>workflow</code> to start or re-run one.
                    </small>
                </label>
                <button
                    type="button"
                    className={`signin-btn${borrowed ? "" : " primary"}`}
                    disabled={!canSubmit}
                    onClick={() => submit(token.trim())}>
                    {busy ? "Checking…" : "Sign in with token"}
                </button>
            </div>

            <SignInError error={attempt.error} />

            <div className="signin-alt">
                <button
                    type="button"
                    className="signin-link"
                    onClick={() => void openUrl(tokenPage(host.trim() || "github.com")).catch(swallow("open GitHub"))}>
                    Create a token
                </button>
                {!editingHost && (
                    <button type="button" className="signin-link" onClick={() => setEditingHost(true)}>
                        GitHub Enterprise
                    </button>
                )}
            </div>
        </SignInScreen>
    );
}
