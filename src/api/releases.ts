import { invokeCommand as invoke } from "./invoke";
import { isRecord } from "../lib/guards";

export interface ReleaseContributor {
    login: string;
    name: string;
    commits: number;
    avatar: string;
}

export interface ReleaseNotes {
    version: string;
    notes: string | null;
    date: string | null;
    commits: number | null;
    compare: string | null;
    contributors: ReleaseContributor[];
}

/** What a release ships about itself in its update manifest. */
export interface ReleaseCredits {
    commits: number;
    compare: string;
    contributors: ReleaseContributor[];
    /** `data:` URLs keyed by avatar address. */
    avatars: Record<string, string>;
}

export interface HeldRelease {
    version: string;
    notes: string | null;
    date: string | null;
    credits: ReleaseCredits | null;
}

function parseContributor(value: unknown): ReleaseContributor | null {
    if (!isRecord(value)) return null;
    const { login, name, commits, avatar } = value;
    if (typeof login !== "string" || typeof name !== "string" || typeof commits !== "number" || typeof avatar !== "string") return null;
    return { login, name, commits, avatar };
}

export function parseReleaseCredits(value: unknown): ReleaseCredits | null {
    if (!isRecord(value) || typeof value.commits !== "number" || typeof value.compare !== "string") return null;
    if (!Array.isArray(value.contributors) || !isRecord(value.avatars)) return null;
    const avatars = Object.fromEntries(Object.entries(value.avatars).filter((entry): entry is [string, string] => typeof entry[1] === "string"));
    return {
        commits: value.commits,
        compare: value.compare,
        contributors: value.contributors.map(parseContributor).filter((person): person is ReleaseContributor => person !== null),
        avatars,
    };
}

export const releasesApi = {
    notes: (version: string) => invoke<ReleaseNotes>("release_notes", { version }),
    avatars: (urls: string[]) => invoke<Record<string, string>>("release_avatars", { urls }),
};
