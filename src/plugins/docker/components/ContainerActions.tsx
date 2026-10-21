import type { ContainerDetail } from "../api";
import { act, openShell, remove } from "../actions";

/** What can be done to the container as it is now, most likely first. */
export function ContainerActions({ container, onRemoved }: { container: ContainerDetail; onRemoved: () => void }) {
    const { status, running } = container.state;
    const paused = status === "paused";
    const name = container.name;
    return (
        <div className="docker-actions" role="group" aria-label="Actions">
            {!running ? (
                <button type="button" className="docker-button primary" onClick={() => void act(name, "start")}>
                    Start
                </button>
            ) : null}
            {running && !paused ? (
                <button type="button" className="docker-button" onClick={() => void act(name, "stop")}>
                    Stop
                </button>
            ) : null}
            {running ? (
                <button type="button" className="docker-button" onClick={() => void act(name, "restart")}>
                    Restart
                </button>
            ) : null}
            {running ? (
                <button type="button" className="docker-button" onClick={() => void act(name, paused ? "unpause" : "pause")}>
                    {paused ? "Resume" : "Pause"}
                </button>
            ) : null}
            {running && !paused ? (
                <button type="button" className="docker-button" onClick={() => openShell(name)}>
                    Open shell
                </button>
            ) : null}
            {running ? (
                <button type="button" className="docker-button danger" onClick={() => void act(name, "kill")}>
                    Kill
                </button>
            ) : null}
            <button
                type="button"
                className="docker-button danger"
                onClick={() =>
                    void remove(name, running).then((removed) => {
                        if (removed) onRemoved();
                    })
                }>
                Remove
            </button>
        </div>
    );
}
