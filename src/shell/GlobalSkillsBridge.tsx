import { useEffect } from "react";
import { skillsApi } from "../api/skills";
import { getIpcTransport } from "../api/transport";
import { notify, reportError, swallow } from "../state/toast";

export const GLOBAL_SKILL_ADDED_EVENT = "global_skill_added";

interface GlobalSkillAdded {
    agent: string;
    name: string;
    path: string;
}

/** Offers to move a skill an agent saved into its own global folder into Sikemux, where every agent gets it. */
export function GlobalSkillsBridge() {
    useEffect(() => {
        const controller = new AbortController();
        getIpcTransport()
            .subscribe<GlobalSkillAdded>(
                GLOBAL_SKILL_ADDED_EVENT,
                ({ payload: { agent, name, path } }) =>
                    notify("info", `${agent} saved the skill ${name} in its own folder, so only ${agent} has it, outside Sikemux too.`, {
                        action: {
                            label: "Move into Sikemux",
                            run: () =>
                                skillsApi
                                    .adopt(path)
                                    .then(() => notify("success", `${name} is in Sikemux now, for every agent's next session.`))
                                    .catch(reportError("Move skill")),
                        },
                    }),
                { signal: controller.signal },
            )
            .catch((error: unknown) => {
                if (!controller.signal.aborted) swallow("skills listener")(error);
            });
        return () => controller.abort();
    }, []);
    return null;
}
