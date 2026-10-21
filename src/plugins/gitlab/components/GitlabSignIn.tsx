import { useResource } from "../../../plugin-api/resources";
import { defineService } from "../../../plugin-api/service";
import { SkeletonRows, TokenSignIn } from "../../../plugin-api/ui";
import { gitlabApi, type GitlabStatus } from "../api";
import { GITLAB_PLUGIN } from "../kinds";
import { GitlabMark } from "./GitlabMark";

/** Where to make a token: the server's own settings page, which is gitlab.com's unless another server is named. */
export function tokensPage(host: string): string {
    const server = host.replace(/^https:\/\//, "").split("/")[0] || "gitlab.com";
    return `https://${server}/-/user_settings/personal_access_tokens?name=Sikemux&scopes=api,read_user`;
}

/** The sign-in form says why GitLab stopped taking the token, which only GitLab's own status says. */
const gitlabStatusR = defineService(GITLAB_PLUGIN, () => gitlabApi.status()).statusR;

export function GitlabSignIn({ onSignedIn }: { onSignedIn: (account: string | null) => void }) {
    const status = useResource(gitlabStatusR);
    if (status.status === "loading") return <SkeletonRows rows={4} label="Connecting to GitLab" />;
    return <SignInForm status={status.data ?? null} onSignedIn={onSignedIn} />;
}

/** A personal access token on gitlab.com, or on a company's own server named above it. */
export function SignInForm({ status, onSignedIn }: { status: GitlabStatus | null; onSignedIn: (account: string | null) => void }) {
    return (
        <TokenSignIn
            status={status}
            service="GitLab"
            mark={<GitlabMark size={26} />}
            lede="Merge requests, pipelines, issues and your To-Do list for this project, right beside your changes."
            fields={[
                {
                    name: "host",
                    label: "Server",
                    placeholder: "gitlab.com",
                    optional: true,
                    initial: status?.host && status.host !== "gitlab.com" ? status.host : "",
                    hint: "Leave empty for gitlab.com, or name your company's server, such as gitlab.acme.dev.",
                },
                {
                    name: "token",
                    label: "Personal access token",
                    placeholder: "glpat-…",
                    secret: true,
                    hint: "The api scope lets Sikemux start and retry pipelines; read_api only reads.",
                },
            ]}
            signIn={async ({ host, token }) => {
                const result = await gitlabApi.signInWithToken(token, host);
                if (!result.ok) return result.message ?? "GitLab turned that token down";
                onSignedIn(result.account);
            }}
            makeToken={({ host }) => ({ label: `Create a token on ${host || "gitlab.com"}`, url: tokensPage(host) })}
        />
    );
}
