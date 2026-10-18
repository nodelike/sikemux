import { useEffect, useState } from "react";
import { IconUser } from "../../../plugin-api/ui";
import { jiraApi, type JiraPerson, type JiraProject } from "../api";

/** The person's Jira picture, or their initials until it loads or when it cannot. */
export function JiraFace({ person }: { person: Pick<JiraPerson, "name" | "avatarUrl"> }) {
    const picture = usePicture(person.avatarUrl);
    if (picture) return <img className="jira-face" src={picture} alt="" draggable={false} />;
    return <>{initials(person.name)}</>;
}

/** A picture Jira holds, once the app has fetched it as a `data:` address. A site's own pictures need its sign-in. */
function usePicture(url: string | null, site?: string): string | null {
    const [picture, setPicture] = useState<{ url: string; data: string } | null>(null);
    useEffect(() => {
        if (!url) return;
        let live = true;
        (site === undefined ? jiraApi.avatar(url) : jiraApi.siteImage(url, site || undefined))
            .then((data) => live && data && setPicture({ url, data }))
            .catch(() => {});
        return () => {
            live = false;
        };
    }, [url, site]);
    return picture && picture.url === url ? picture.data : null;
}

/** A project's square picture, or its first letter until it loads or when it cannot. */
export function ProjectAvatar({ project, site }: { project: JiraProject; site: string }) {
    const picture = usePicture(project.avatarUrl, site);
    return (
        <span className="jira-project-avatar" aria-hidden="true">
            {picture ? <img src={picture} alt="" draggable={false} /> : (project.name[0] ?? project.key[0] ?? "?").toUpperCase()}
        </span>
    );
}

function initials(name: string): string {
    return name
        .split(/\s+/)
        .filter(Boolean)
        .slice(0, 2)
        .map((part) => part[0]?.toUpperCase() ?? "")
        .join("");
}

/** A person's round picture, or an empty one for nobody. */
export function Avatar({ person, large }: { person: Pick<JiraPerson, "name" | "avatarUrl"> | null; large?: boolean }) {
    const size = large ? " large" : "";
    if (!person)
        return (
            <span className={`jira-avatar nobody${size}`} title="Unassigned">
                <IconUser size={large ? 13 : 11} />
            </span>
        );
    return (
        <span className={`jira-avatar${size}`} title={person.name}>
            <JiraFace person={person} />
        </span>
    );
}
