import { defineService } from "../../plugin-api/service";
import { jiraApi, type JiraBoardPage, type JiraIssueList, type JiraIssueListKind, type JiraIssueView, type JiraSpaces } from "./api";
import { JIRA_PLUGIN } from "./kinds";

export const jira = defineService(JIRA_PLUGIN, jiraApi.status);

/** The site shown, or the default one when empty. */
const siteOf = (site: string) => site || undefined;

export const jiraListV = jira.view<[list: JiraIssueListKind, site: string], JiraIssueList>("listView", (list, site) => ({
    list,
    site: siteOf(site),
}));

export const jiraIssueV = jira.view<[key: string, site: string], JiraIssueView>("issueView", (key, site) => ({ key, site: siteOf(site) }));

export const jiraSpacesV = jira.view<[site: string], JiraSpaces>("spacesView", (site) => ({ site: siteOf(site) }));

/** A board, narrowed to the cards of the people picked: account ids, plus "me" and "none" for nobody. */
export const jiraBoardV = jira.view<[id: number, site: string, picked: string], JiraBoardPage>("boardView", (id, site, picked) => {
    const people = picked ? picked.split(",") : [];
    return {
        id,
        site: siteOf(site),
        people: people.filter((each) => each !== "me" && each !== "none"),
        mine: people.includes("me"),
        unassigned: people.includes("none"),
    };
});
