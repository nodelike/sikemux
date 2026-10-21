import { TokenSignIn } from "../../../plugin-api/ui";
import { linearApi, type LinearStatus } from "../api";
import { LinearMark } from "./LinearMark";

/** Where a person makes the personal API key they paste here. */
export const LINEAR_KEYS_PAGE = "https://linear.app/settings/account/security";

/** Signing in through the browser comes first; a personal API key is for workspaces that turn outside apps away. */
export function LinearSignIn({ status, onSignedIn }: { status: LinearStatus | undefined; onSignedIn: (status: LinearStatus) => void }) {
    return (
        <TokenSignIn
            status={status}
            service="Linear"
            mark={<LinearMark size={26} className="icon-linear" />}
            lede="See the issues you are meant to be working on beside your code, and let agents read, comment on and move them."
            fields={[
                {
                    name: "key",
                    label: "Personal API key",
                    placeholder: "lin_api_…",
                    secret: true,
                    hint: <>In Linear, open Settings → Security &amp; access, make a personal API key, and paste it here.</>,
                },
            ]}
            signIn={async ({ key }) => onSignedIn(await linearApi.signIn(key))}
            makeToken={() => ({ label: "Make an API key", url: LINEAR_KEYS_PAGE })}
            browser={{
                label: "Continue with Linear",
                icon: <LinearMark size={14} />,
                start: linearApi.signInWithBrowser,
                done: onSignedIn,
                tokenInstead: "Use an API key instead",
            }}
        />
    );
}
