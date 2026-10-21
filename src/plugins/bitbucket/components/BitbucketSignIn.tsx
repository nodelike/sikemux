import { useResource } from "../../../plugin-api/resources";
import { defineService } from "../../../plugin-api/service";
import { SkeletonRows, TokenSignIn } from "../../../plugin-api/ui";
import { bitbucketApi, type BitbucketStatus } from "../api";
import { BITBUCKET_PLUGIN } from "../kinds";
import { BitbucketMark } from "./BitbucketMark";

const API_TOKENS_PAGE = "https://id.atlassian.com/manage-profile/security/api-tokens";

/** The sign-in form wants to know whether this build can sign in through the browser, which only Bitbucket's own status says. */
const bitbucketStatusR = defineService(BITBUCKET_PLUGIN, () => bitbucketApi.status()).statusR;

export function BitbucketSignIn({ onSignedIn }: { onSignedIn: (account: string | null) => void }) {
    const status = useResource(bitbucketStatusR);
    if (status.status === "loading") return <SkeletonRows rows={4} label="Connecting to Bitbucket" />;
    return <SignInForm status={status.data} onSignedIn={onSignedIn} />;
}

/** Signing in through the browser comes first; a pasted token is for workspaces that turn outside apps away. */
export function SignInForm({ status, onSignedIn }: { status: BitbucketStatus | undefined; onSignedIn: (account: string | null) => void }) {
    const signedIn = (result: BitbucketStatus, refused: string) => {
        if (!result.ok) return result.message ?? refused;
        onSignedIn(result.account);
    };
    return (
        <TokenSignIn
            status={status}
            service="Bitbucket"
            mark={<BitbucketMark size={26} />}
            lede="Pull requests, pipelines and reviews for this repository, right beside your changes."
            fields={[
                { name: "token", label: "Token", placeholder: "ATATT… or ATCTT…", secret: true },
                {
                    name: "email",
                    label: "Atlassian account email",
                    placeholder: "you@example.com",
                    email: true,
                    optional: true,
                    hint: "An API token needs the email it belongs to. Leave it empty for a repository or workspace access token.",
                },
            ]}
            signIn={async ({ token, email }) =>
                signedIn(await bitbucketApi.signInWithToken(token, email || null), "Bitbucket turned that token down")
            }
            makeToken={() => ({ label: "Create an API token", url: API_TOKENS_PAGE })}
            browser={
                status?.browserSignIn
                    ? {
                          label: "Continue with Bitbucket",
                          icon: <BitbucketMark size={14} mono />,
                          start: bitbucketApi.signInWithBrowser,
                          done: (result) => signedIn(result, "Bitbucket did not let that account in"),
                          tokenInstead: "Use a token instead",
                      }
                    : undefined
            }
        />
    );
}
