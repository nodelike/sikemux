import { TokenSignIn } from "../../../plugin-api/ui";
import { vercelApi, type VercelStatus } from "../api";
import { VercelMark } from "./VercelMark";

/** Where a person makes the access token they paste here. */
export const VERCEL_TOKENS_PAGE = "https://vercel.com/account/settings/tokens";

/** Signing in through the browser installs the Sikemux integration on a team; an access token is for builds without it. */
export function VercelSignIn({ status, onSignedIn }: { status: VercelStatus | undefined; onSignedIn: (status: VercelStatus) => void }) {
    return (
        <TokenSignIn
            status={status}
            service="Vercel"
            mark={<VercelMark size={26} className="icon-vercel" />}
            lede="See your projects, deployments and build logs beside your code, and let agents read them, redeploy and promote."
            fields={[
                {
                    name: "token",
                    label: "Access token",
                    placeholder: "Paste a Vercel token",
                    secret: true,
                    hint: "In Vercel, open Account Settings → Tokens, create a token for the scopes you want, and paste it here.",
                },
            ]}
            signIn={async ({ token }) => onSignedIn(await vercelApi.signIn(token))}
            makeToken={() => ({ label: "Make a token", url: VERCEL_TOKENS_PAGE })}
            browser={
                status?.browserSignIn
                    ? {
                          label: "Continue with Vercel",
                          icon: <VercelMark size={14} />,
                          start: vercelApi.signInWithBrowser,
                          done: onSignedIn,
                          tokenInstead: "Use an access token instead",
                      }
                    : undefined
            }
        />
    );
}
