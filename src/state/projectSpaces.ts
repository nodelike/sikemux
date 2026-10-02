import type { ProjectSpace, Session } from "./types";

export const MAX_SPACE_NAME_LENGTH = 40;

/** All shows every project; a space shows only the projects put in it. */
export function isProjectShown(cwd: string, projectSpaces: Readonly<Record<string, string>>, activeSpaceId: string | null): boolean {
    return activeSpaceId === null || projectSpaces[cwd] === activeSpaceId;
}

export function shownProjects<T extends Pick<Session, "cwd">>(
    projects: readonly T[],
    projectSpaces: Readonly<Record<string, string>>,
    activeSpaceId: string | null,
): T[] {
    return projects.filter((project) => isProjectShown(project.cwd, projectSpaces, activeSpaceId));
}

/** The first character as a person sees it, so an emoji made of several code points stays whole. */
export function firstGrapheme(text: string): string {
    const first = new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(text.trim())[Symbol.iterator]().next();
    return first.done ? "" : first.value.segment;
}

export const spaceBadge = (space: ProjectSpace): string => space.icon || firstGrapheme(space.name).toUpperCase();

export const spaceName = (name: string): string => name.trim().slice(0, MAX_SPACE_NAME_LENGTH);
