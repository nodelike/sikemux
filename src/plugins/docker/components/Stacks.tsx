import { basename } from "../../../plugin-api/host";
import type { Stack } from "../api";
import { actOnStack, runCompose } from "../compose";
import { isRunning } from "../format";
import { Dot, StateDot } from "./parts";

export function Stacks({ stacks, find, onOpen }: { stacks: Stack[]; find: string; onOpen: (container: string) => void }) {
    const words = find.trim().toLowerCase();
    const shown = words
        ? stacks.filter(
              (stack) => stack.project.toLowerCase().includes(words) || stack.services.some((service) => service.name.toLowerCase().includes(words)),
          )
        : stacks;
    if (stacks.length === 0) {
        return <p className="docker-note">No Compose stacks. Containers started with docker compose up show here, grouped by project.</p>;
    }
    if (shown.length === 0) return <p className="docker-note">No stacks match.</p>;
    return (
        <div className="docker-stacks">
            {shown.map((stack) => (
                <StackCard key={stack.project} stack={stack} onOpen={onOpen} />
            ))}
        </div>
    );
}

function StackCard({ stack, onOpen }: { stack: Stack; onOpen: (container: string) => void }) {
    const up = stack.running > 0;
    const whole = stack.running === stack.total;
    return (
        <section className="docker-facts docker-stack" aria-label={stack.project}>
            <header className="docker-stack-head">
                <Dot tone={whole ? "live" : up ? "warn" : "idle"} />
                <h3 className="docker-stack-name">{stack.project}</h3>
                <span className="docker-muted">
                    {stack.running} of {stack.total} running
                </span>
                <div className="docker-actions" role="group" aria-label={`${stack.project} actions`}>
                    {stack.configFiles.length > 0 ? (
                        <>
                            <button
                                type="button"
                                className={`docker-button${whole ? "" : " primary"}`}
                                onClick={() => runCompose(stack, "Up", "up", "-d")}>
                                Up
                            </button>
                            <button type="button" className="docker-button" onClick={() => runCompose(stack, "Rebuild", "up", "-d", "--build")}>
                                Rebuild
                            </button>
                        </>
                    ) : null}
                    {up ? (
                        <>
                            <button type="button" className="docker-button" onClick={() => void actOnStack(stack, "restart")}>
                                Restart
                            </button>
                            <button type="button" className="docker-button" onClick={() => void actOnStack(stack, "stop")}>
                                Stop
                            </button>
                        </>
                    ) : (
                        <button type="button" className="docker-button" onClick={() => void actOnStack(stack, "start")}>
                            Start
                        </button>
                    )}
                    <button type="button" className="docker-button danger" onClick={() => void actOnStack(stack, "down")}>
                        Down
                    </button>
                </div>
            </header>
            {stack.workingDir ? (
                <p className="docker-stack-where docker-mono docker-muted" title={stack.configFiles.join("\n")}>
                    {stack.workingDir}
                    {stack.configFiles.length > 0 ? ` · ${stack.configFiles.map(basename).join(", ")}` : ""}
                </p>
            ) : null}
            <ul className="docker-services">
                {stack.services.map((service) => (
                    <li key={service.name} className="docker-service">
                        <span className="docker-service-name">{service.name}</span>
                        <span className="docker-service-containers">
                            {service.containers.map((container) => (
                                <button
                                    key={container.id}
                                    type="button"
                                    className={`docker-service-container${isRunning(container) ? "" : " stopped"}`}
                                    onClick={() => onOpen(container.name)}>
                                    <StateDot container={container} />
                                    {container.name}
                                    <span className="docker-muted">{container.status}</span>
                                </button>
                            ))}
                        </span>
                    </li>
                ))}
            </ul>
        </section>
    );
}
