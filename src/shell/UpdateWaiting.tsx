import { useState } from "react";
import { cancelWaitingUpdate, forceUpdate, waitingLabel } from "../api/updater";
import { useStore } from "../state/store";

/** An update held back by agents that cannot be carried over it yet, with a
 *  way to install now that names the agents it would stop. */
export function UpdateWaiting() {
    const blockers = useStore((s) => s.updateBlockers);
    const [confirming, setConfirming] = useState(false);
    const count = blockers.length;
    const them = count === 1 ? "it" : "them";
    return (
        <section className="wn-waiting" aria-live="polite">
            <p>
                <b>{waitingLabel(count)}</b>. Agents at work keep running through an update, but {count === 1 ? "this one is" : "these are"} still
                starting or in the middle of a request. The update installs on its own once {count === 1 ? "it is" : "they are"} ready.
            </p>
            {confirming && (
                <>
                    <ul className="wn-waiting-agents">
                        {blockers.map((blocker) => (
                            <li key={blocker.agentId}>
                                <span>{blocker.title ?? blocker.agentId}</span>
                                <small>
                                    {blocker.provider} · {blocker.reason === "starting" ? "starting" : "busy"}
                                </small>
                            </li>
                        ))}
                    </ul>
                    <p className="wn-status">
                        Updating now stops {them} and starts {them} again on the same chat. Whatever {count === 1 ? "it was" : "they were"} doing
                        stops.
                    </p>
                </>
            )}
            <div className="wn-waiting-actions">
                <button type="button" onClick={confirming ? () => setConfirming(false) : cancelWaitingUpdate}>
                    {confirming ? "Keep waiting" : "Not now"}
                </button>
                <button type="button" className="danger" onClick={confirming ? () => void forceUpdate() : () => setConfirming(true)}>
                    {confirming ? `Stop ${count === 1 ? "it" : `${count} agents`} and update` : "Force update"}
                </button>
            </div>
        </section>
    );
}
