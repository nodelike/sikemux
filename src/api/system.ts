import { coreCall } from "./core";
import { invokeCommand as invoke } from "./invoke";

/** Opens `url` in the person's browser, or in `app` when one is named. */
export const openUrl = (url: string, app: string | null = null, shortcut: string | null = null): Promise<void> =>
    invoke<void>("open_url", { url, app, shortcut });

/** The shell new terminals start, and which tools this host has. */
export interface HostTools {
    shell: string;
    git: boolean;
    aws: boolean;
    rnd: boolean;
}

export const hostTools = (): Promise<HostTools> => coreCall<HostTools>("hostTools");
