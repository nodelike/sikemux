import { invokeCommand as invoke } from "./invoke";

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

export const releasesApi = {
    notes: (version: string) => invoke<ReleaseNotes>("release_notes", { version }),
    avatars: (urls: string[]) => invoke<Record<string, string>>("release_avatars", { urls }),
};

export const openInBrowser = (url: string) => invoke<void>("open_url", { url, app: null, shortcut: null });
