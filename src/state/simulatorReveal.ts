import { useEffect } from "react";
import { simulatorApi } from "../api/simulator";
import * as cmd from "./commands";
import { deskItemsOf, simulatorKey } from "./desks";
import { getState } from "./store";

/** Puts each simulator an agent attaches on that agent's desk, live, and takes it off when the agent lets go. */
export function useSimulatorReveal(): void {
    useEffect(() => {
        const controller = new AbortController();
        void simulatorApi
            .subscribeAttached(({ agentId, ...simulator }) => cmd.openDeskSimulator(agentId, simulator), controller.signal)
            .catch(() => {});
        void simulatorApi
            .subscribeDetached(({ agentId, udid }) => {
                const item = deskItemsOf(getState(), agentId).find((candidate) => candidate.key === simulatorKey(udid));
                if (item) cmd.closeDeskItem(agentId, item);
            }, controller.signal)
            .catch(() => {});
        return () => controller.abort();
    }, []);
}
