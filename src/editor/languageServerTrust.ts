import { basename } from "../lib/paths";
import { setLanguageServerTrust } from "../state/commands";
import { confirmDialog, type ConfirmRequest } from "../state/dialog";
import { getState } from "../state/store";

const asking = new Map<string, Promise<boolean>>();

/** Asks the first time a project would start a language server, and remembers the answer. */
export function languageServersAllowed(project: string, ask: (request: ConfirmRequest) => Promise<boolean> = confirmDialog): Promise<boolean> {
    const decided = getState().languageServerTrust[project];
    if (decided !== undefined) return Promise.resolve(decided);
    const pending = asking.get(project);
    if (pending) return pending;
    const answer = ask({
        title: `Start language servers for ${basename(project)}?`,
        body:
            "Language servers can run code from this project, such as its build scripts and installed tools. Only allow this for projects you trust.\n" +
            "You can change this later from the command palette.",
        confirmLabel: "Allow",
        cancelLabel: "Don't allow",
    })
        .then((allowed) => {
            setLanguageServerTrust(project, allowed);
            return allowed;
        })
        .finally(() => asking.delete(project));
    asking.set(project, answer);
    return answer;
}
