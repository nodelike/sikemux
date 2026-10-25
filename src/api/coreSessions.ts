import { invokeCommand as invoke } from "./invoke";
import type { SessionInfo } from "@shared/wire.generated";

export const coreSessionsApi = {
    /** This host's sessions, or those of `host`, another the app joined. */
    list: (host?: string) => invoke<SessionInfo[]>("pty_sessions", host ? { host } : undefined),
    kill: (id: number) => invoke<void>("pty_kill", { id }),
    quitAndStopEverything: () => invoke<void>("app_quit_and_stop_everything"),
};
