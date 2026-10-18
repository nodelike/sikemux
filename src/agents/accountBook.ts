import { agentApi, type AgentAccountBook } from "../api/agents";
import { adoptAgentAccounts } from "../state/persist";
import { getState, useStore, type StoreState } from "../state/store";
import { swallow } from "../state/toast";

/** How often a core that could not be asked at start is asked again. */
const RETRY_MS = 5_000;

function bookOf(state: StoreState): AgentAccountBook {
    return { profiles: state.providerProfiles, selected: state.selectedProviderProfileIds, autoSwitch: state.accountAutoSwitch };
}

/** Sends the person's agent accounts to the core whenever they change here. */
function follow(): () => void {
    let sent = JSON.stringify(bookOf(getState()));
    return useStore.subscribe((state, previous) => {
        if (
            state.providerProfiles === previous.providerProfiles &&
            state.selectedProviderProfileIds === previous.selectedProviderProfileIds &&
            state.accountAutoSwitch === previous.accountAutoSwitch
        )
            return;
        const book = bookOf(state);
        const json = JSON.stringify(book);
        if (json === sent) return;
        sent = json;
        agentApi.setAccounts(book).catch(swallow("save agent accounts in the core"));
    });
}

/**
 * Keeps the core's agent accounts in step with edits made here. `known` is what the core said at start, or `undefined`
 * when it could not be asked, so it is asked again and its accounts taken up once it answers.
 */
export function syncAgentAccounts(known: AgentAccountBook | null | undefined): () => void {
    if (known !== undefined) return follow();
    let stop = () => {};
    const retry = window.setInterval(() => {
        agentApi.accounts().then(
            (book) => {
                window.clearInterval(retry);
                if (book) adoptAgentAccounts(book);
                stop = follow();
            },
            () => {},
        );
    }, RETRY_MS);
    return () => {
        window.clearInterval(retry);
        stop();
    };
}
