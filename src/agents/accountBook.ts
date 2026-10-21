import { agentApi, type AgentAccountBook } from "../api/agents";
import { onCoreChanged } from "../api/coreChanged";
import { getIpcTransport } from "../api/transport";
import { adoptAgentAccounts } from "../state/persist";
import { swallow } from "../state/toast";

function reread(): void {
    agentApi
        .accounts()
        .then((book) => book && adoptAgentAccounts(book))
        .catch(swallow("read the agent accounts from the core"));
}

/**
 * Keeps the person's agent accounts as the core has them, reading them again whenever any client changes them
 * and whenever the core connects. `known` is what the core said at start, or `undefined` when it could not be asked.
 */
export function syncAgentAccounts(known: AgentAccountBook | null | undefined): () => void {
    const stop = onCoreChanged("agentAccounts", reread);
    const connected = new AbortController();
    void getIpcTransport().subscribe("core_connected", reread, { signal: connected.signal }).catch(swallow("follow the core connecting"));
    if (known === undefined) reread();
    return () => {
        connected.abort();
        stop();
    };
}
