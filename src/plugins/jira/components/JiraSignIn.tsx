import { TokenSignIn } from "../../../plugin-api/ui";
import { jiraApi, type JiraStatus } from "../api";
import { JiraMark } from "./JiraMark";

const API_TOKENS_PAGE = "https://id.atlassian.com/manage-profile/security/api-tokens";

/** Signing in through the browser comes first; an API token is for builds without it and sites that turn outside apps away. */
export function JiraSignIn({ status, onSignedIn }: { status: JiraStatus | undefined; onSignedIn: (status: JiraStatus) => void }) {
    return (
        <TokenSignIn
            status={status}
            service="Jira"
            mark={<JiraMark size={26} />}
            lede="The tickets you are working on, beside the code, and tools for your agents to read and update them."
            fields={[
                { name: "site", label: "Jira site", placeholder: "your-team.atlassian.net" },
                { name: "email", label: "Atlassian account email", placeholder: "you@example.com", email: true },
                { name: "token", label: "API token", placeholder: "ATATT…", secret: true },
            ]}
            signIn={async ({ site, email, token }) => onSignedIn(await jiraApi.signIn(site, email, token))}
            makeToken={() => ({ label: "Create an API token", url: API_TOKENS_PAGE })}
            browser={
                status?.browserSignIn
                    ? {
                          label: "Continue with Atlassian",
                          icon: <JiraMark size={14} mono />,
                          start: jiraApi.signInWithBrowser,
                          done: onSignedIn,
                          tokenInstead: "Use an API token instead",
                      }
                    : undefined
            }
        />
    );
}
