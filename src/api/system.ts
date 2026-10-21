import { invokeCommand as invoke } from "./invoke";

/** The shell new terminals start, and which tools this machine has. */
export interface IntegrationHealth {
    shell: string;
    git: boolean;
    aws: boolean;
    rnd: boolean;
}

/** Opens `url` in the person's browser, or in `app` when one is named. */
export const openUrl = (url: string, app: string | null = null, shortcut: string | null = null): Promise<void> =>
    invoke<void>("open_url", { url, app, shortcut });

export const integrationHealth = (): Promise<IntegrationHealth> => invoke<IntegrationHealth>("integration_health");
