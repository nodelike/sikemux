import { openUrl, swallow } from "../../../plugin-api/host";
import type { Container, Port } from "../api";
import { portLabel, portUrl, toneOf, type Tone } from "../format";

export function Dot({ tone, label }: { tone: Tone; label?: string }) {
    return <span className={`docker-dot ${tone}`} role={label ? "img" : undefined} aria-label={label} aria-hidden={label ? undefined : true} />;
}

export function StateDot({ container }: { container: Pick<Container, "state" | "status"> }) {
    return <Dot tone={toneOf(container)} label={container.status || container.state} />;
}

/** Published ports open in the browser; the rest are only listed. */
export function Ports({ ports }: { ports: Port[] }) {
    if (ports.length === 0) return <span className="docker-muted">—</span>;
    return (
        <span className="docker-ports">
            {ports.map((port) => {
                const url = portUrl(port);
                const label = portLabel(port);
                return url ? (
                    <button
                        key={label}
                        type="button"
                        className="docker-port link"
                        title={`Open ${url} → ${port.private}`}
                        onClick={(event) => {
                            event.stopPropagation();
                            void openUrl(url).catch(swallow("open the port"));
                        }}>
                        {label}
                    </button>
                ) : (
                    <span key={label} className="docker-port">
                        {label}
                    </span>
                );
            })}
        </span>
    );
}
