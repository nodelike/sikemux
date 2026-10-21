import { useResource } from "../../../plugin-api/resources";
import { defineService } from "../../../plugin-api/service";
import { SkeletonRows, TokenSignIn } from "../../../plugin-api/ui";
import { actionsApi, type ActionsStatus } from "../api";
import { GITHUB_PLUGIN } from "../kinds";
import { GithubMark } from "./GithubMark";

/** The sign-in form wants to know where a borrowed token came from, which only GitHub's own status says. */
const githubStatusR = defineService(GITHUB_PLUGIN, () => actionsApi.status()).statusR;

export function GithubSignIn({ onSignedIn }: { onSignedIn: (account: string | null) => void }) {
    const status = useResource(githubStatusR);
    if (status.status === "loading") return <SkeletonRows rows={4} label="Connecting to GitHub" />;
    return <ActionsSignIn status={status.data} onSignedIn={onSignedIn} />;
}

function borrowed(status: ActionsStatus | undefined): { label: string; note: string } | null {
    if (status?.tokenSource === "ghCli")
        return { label: "Continue with the gh CLI", note: "The gh CLI is signed in here, so Sikemux uses its token." };
    if (status?.tokenSource !== "environment") return null;
    const variable = status.tokenVariable ?? "that token";
    return {
        label: `Continue with ${variable}`,
        note: `${status.tokenVariable ?? "A token variable"} is set in your shell, so Sikemux uses that token.`,
    };
}

/**
 * The token is the only thing to ask for, and often there is not even that:
 * a shell that exports one, or a signed-in `gh`, is offered as it is.
 */
export function ActionsSignIn({ status, onSignedIn }: { status: ActionsStatus | undefined; onSignedIn: (account: string | null) => void }) {
    const host = status?.host ?? "github.com";
    const signIn = async (onHost: string, token?: string) => {
        const result = await actionsApi.signIn(onHost, token);
        if (!result.ok) return result.message ?? "GitHub turned that token down";
        onSignedIn(result.account);
    };
    const found = borrowed(status);
    return (
        <TokenSignIn
            status={status}
            service="GitHub"
            mark={<GithubMark size={26} />}
            lede="Pull requests, Actions runs and issues for this repository, right beside your changes."
            found={found && { ...found, icon: <GithubMark size={14} />, use: () => signIn(host) }}
            fields={[
                {
                    name: "host",
                    label: "Host",
                    placeholder: "github.com",
                    optional: true,
                    initial: host,
                    revealedBy: host === "github.com" ? "GitHub Enterprise" : undefined,
                },
                {
                    name: "token",
                    label: "Personal access token",
                    placeholder: "ghp_… or github_pat_…",
                    secret: true,
                    hint: (
                        <>
                            It needs <code>repo</code> to read private repositories and <code>workflow</code> to start or re-run one.
                        </>
                    ),
                },
            ]}
            submitLabel="Sign in with token"
            signIn={({ host: onHost, token }) => signIn(onHost, token)}
            makeToken={({ host: onHost }) => ({
                label: "Create a token",
                url: `https://${onHost || "github.com"}/settings/tokens/new?scopes=repo,workflow&description=Sikemux`,
            })}
        />
    );
}
