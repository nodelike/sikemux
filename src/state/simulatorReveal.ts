import { useEffect } from "react";
import { simApi } from "../api/sim";
import * as cmd from "./commands";
import { noteSimulatorActing, noteSimulatorAttached, noteSimulatorDetached, setSimulatorAttachments } from "./simulatorAgents";

/**
 * Shows each device an agent attaches on that agent's desk, so the person
 * watches what it does, and keeps track of which agent holds which device.
 */
export function useSimulatorReveal(): void {
    useEffect(() => {
        const controller = new AbortController();
        const readAttachments = () =>
            void simApi.attachments().then(
                (list) => !controller.signal.aborted && setSimulatorAttachments(list),
                () => {},
            );
        readAttachments();
        void simApi
            .subscribeAttached((attached) => {
                noteSimulatorAttached(attached);
                cmd.openDeskSimulator(attached.agentId, { focus: false, device: { udid: attached.udid, name: attached.name } });
                readAttachments();
            }, controller.signal)
            .catch(() => {});
        void simApi
            .subscribeDetached(({ agentId }) => {
                noteSimulatorDetached(agentId);
                readAttachments();
            }, controller.signal)
            .catch(() => {});
        void simApi.subscribeActing(({ agentId, acting }) => noteSimulatorActing(agentId, acting), controller.signal).catch(() => {});
        return () => controller.abort();
    }, []);
}
