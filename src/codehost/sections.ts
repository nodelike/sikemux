import type { CodeHost } from "./registry";
import { SECTIONS, type Section } from "./state";

export function sectionLabel(host: CodeHost, section: Section): string {
    switch (section) {
        case "pulls":
            return "Pull requests";
        case "actions":
            return host.ciName;
        case "issues":
            return "Issues";
        case "releases":
            return "Releases";
        case "inbox":
            return "Inbox";
    }
}

/** The sections this host has, in the order they sit in the git pane's header. */
export function sectionsOf(host: CodeHost): Section[] {
    const { capabilities } = host;
    return SECTIONS.filter((section) =>
        section === "issues" ? capabilities.issues : section === "releases" ? capabilities.releases : section === "inbox" ? capabilities.inbox : true,
    );
}
