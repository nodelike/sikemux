import { TokenSignIn } from "../../../plugin-api/ui";
import { slackApi, type SlackStatus } from "../api";
import { SlackMark } from "./SlackMark";

/** Where a person makes the Slack app whose token they paste here. */
export const SLACK_APPS_PAGE = "https://api.slack.com/apps";

/** A user token from a Slack app installed in the person's workspace; it reads and posts as them. */
export function SlackSignIn({ status, onSignedIn }: { status: SlackStatus | undefined; onSignedIn: (status: SlackStatus) => void }) {
    return (
        <TokenSignIn
            status={status}
            service="Slack"
            mark={<SlackMark size={26} />}
            lede="Read a thread beside your code, hand it to an agent, and let the agent reply in it when the work is done."
            fields={[
                {
                    name: "token",
                    label: "User OAuth Token",
                    placeholder: "xoxp-…",
                    secret: true,
                    hint: (
                        <>
                            Make a Slack app, add the user scopes channels:history, groups:history, im:history, mpim:history, channels:read,
                            groups:read, im:read, mpim:read, chat:write, users:read, users:read.email and search:read, install it to your workspace,
                            and copy the User OAuth Token from OAuth &amp; Permissions.
                        </>
                    ),
                },
            ]}
            signIn={async ({ token }) => onSignedIn(await slackApi.signIn(token))}
            makeToken={() => ({ label: "Make a Slack app", url: SLACK_APPS_PAGE })}
        />
    );
}
