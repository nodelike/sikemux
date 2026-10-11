import type { Session } from "./types";

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

export const SPACE_ICONS = [
    "folder",
    "user",
    "globe",
    "laptop",
    "building",
    "home",
    "briefcase",
    "code",
    "flask",
    "rocket",
    "star",
    "heart",
    "book",
    "bolt",
    "leaf",
    "cube",
    "terminal",
    "music",
] as const;

export type SpaceIcon = (typeof SPACE_ICONS)[number];

export const DEFAULT_SPACE_ICON: SpaceIcon = "folder";

export const isSpaceIcon = (value: unknown): value is SpaceIcon => SPACE_ICONS.includes(value as SpaceIcon);

export const spaceName = (name: string): string => name.trim().slice(0, MAX_SPACE_NAME_LENGTH);
