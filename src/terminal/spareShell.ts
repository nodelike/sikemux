import { coreCall } from "../api/core";
import type { PtyContext, Session } from "../state/types";
import { offerResumableSessions } from "./sessionResume";

/* A new tab takes over the shell the core started ahead for the project in
   front, with that shell's ids, so it never waits on the person's rc files. */

export interface SpareShell {
    readonly windowId: string;
    readonly paneId: string;
    readonly ptyId: number;
}

export function terminalContext(session: Session, windowId: string, paneId: string): PtyContext {
    return {
        sessionId: session.id,
        sessionName: session.name,
        sessionKind: session.kind,
        ...(session.kind === "project" && session.cwd ? { project: session.cwd } : {}),
        windowId,
        paneId,
        shellIntegration: session.kind === "project" || session.kind === "command",
    };
}

/** The shell the core started ahead for `session`'s project, if one is ready. */
export async function takeSpareShell(session: Session): Promise<SpareShell | null> {
    const taken = await coreCall<{ id: number; windowId: string; paneId: string } | null>("takeSpareShell", { project: session.id }).catch(
        () => null,
    );
    if (!taken) return null;
    offerResumableSessions([taken.id]);
    return { windowId: taken.windowId, paneId: taken.paneId, ptyId: taken.id };
}
