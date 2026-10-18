import { useState, type KeyboardEvent } from "react";
import { openUrl, swallow } from "../../../plugin-api/host";
import { resource, useResource } from "../../../plugin-api/resources";
import { SignInError, SignInScreen, SkeletonRows, useSignInAttempt } from "../../../plugin-api/ui";
import { gitlabApi, type GitlabStatus } from "../api";
import { GitlabMark } from "./GitlabMark";

/** Where to make a token: the server's own settings page, which is gitlab.com's unless another server is named. */
export function tokensPage(host: string): string {
    const server =
        host
            .trim()
            .replace(/^https:\/\//, "")
            .split("/")[0] || "gitlab.com";
    return `https://${server}/-/user_settings/personal_access_tokens?name=Sikemux&scopes=api,read_user`;
}

/** The sign-in form says why GitLab stopped taking the token, which only GitLab's own status says. */
const gitlabStatusR = resource({
    kind: "host.gitlabStatus",
    fetch: (): Promise<GitlabStatus> => gitlabApi.status(),
    staleAfterMs: 60_000,
});

export function GitlabSignIn({ onSignedIn }: { onSignedIn: (account: string | null) => void }) {
    const status = useResource(gitlabStatusR);
    if (!status.data && status.status === "loading") return <SkeletonRows rows={4} label="Connecting to GitLab" />;
    return <SignInForm status={status.data ?? null} onSignedIn={onSignedIn} />;
}

/** A personal access token on gitlab.com, or on a company's own server named above it. */
export function SignInForm({ status, onSignedIn }: { status: GitlabStatus | null; onSignedIn: (account: string | null) => void }) {
    const [host, setHost] = useState(status?.host && status.host !== "gitlab.com" ? status.host : "");
    const [token, setToken] = useState("");
    const attempt = useSignInAttempt(status);
    const busy = attempt.busy;

    const canSubmit = !busy && !!token.trim();
    const submit = () => {
        if (!canSubmit) return;
        attempt.run(async () => {
            const result = await gitlabApi.signInWithToken(token.trim(), host.trim());
            if (!result.ok) return result.message ?? "GitLab turned that token down";
            onSignedIn(result.account);
        });
    };
    const onEnter = (event: KeyboardEvent) => {
        if (event.key === "Enter") submit();
    };

    return (
        <SignInScreen
            mark={<GitlabMark size={26} />}
            title="Connect GitLab"
            lede="Merge requests, pipelines, issues and your To-Do list for this project, right beside your changes."
            foot="Sikemux keeps your token in the macOS Keychain.">
            <div className="signin-form">
                <label className="signin-field">
                    <span>Server</span>
                    <input
                        className="signin-input"
                        value={host}
                        onChange={(event) => setHost(event.target.value)}
                        onKeyDown={onEnter}
                        placeholder="gitlab.com"
                        spellCheck={false}
                        autoCapitalize="off"
                        autoCorrect="off"
                    />
                    <small className="signin-hint">Leave empty for gitlab.com, or name your company's server, such as gitlab.acme.dev.</small>
                </label>
                <label className="signin-field">
                    <span>Personal access token</span>
                    <input
                        className="signin-input mono"
                        type="password"
                        value={token}
                        onChange={(event) => setToken(event.target.value)}
                        onKeyDown={onEnter}
                        placeholder="glpat-…"
                        autoFocus
                        spellCheck={false}
                    />
                    <small className="signin-hint">The api scope lets Sikemux start and retry pipelines; read_api only reads.</small>
                </label>
                <button type="button" className="signin-btn primary" disabled={!canSubmit} onClick={submit}>
                    {busy ? "Checking…" : "Sign in"}
                </button>
            </div>

            <SignInError error={attempt.error} />

            <div className="signin-alt">
                <button type="button" className="signin-link" onClick={() => void openUrl(tokensPage(host)).catch(swallow("open GitLab"))}>
                    Create a token on {host.trim() || "gitlab.com"}
                </button>
            </div>
        </SignInScreen>
    );
}
