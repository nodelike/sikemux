import { notify, openUrl, promptDialog, runInTerminal } from "../../../plugin-api/host";
import type { Ingress, Scope, Service } from "../api";
import { kubectlScope } from "./PodView";

/** The port a Service answers on, from a label such as 80→8080/TCP. */
export function servicePort(label: string): number | null {
    const port = Number(/^\d+/.exec(label)?.[0]);
    return Number.isInteger(port) && port > 0 ? port : null;
}

/** A local port to offer for forwarding: the same one when unprivileged, else one above 8000. */
export function localPortFor(port: number): number {
    return port >= 1024 ? port : 8000 + port;
}

export function routeUrl(route: { host: string; path: string }, tls: string[]): string | null {
    if (route.host === "*" || route.host.includes("*")) return null;
    return `${tls.includes(route.host) ? "https" : "http"}://${route.host}${route.path}`;
}

async function forward(scope: Scope, service: Service, port: number) {
    const typed = await promptDialog({
        title: `Forward ${service.name}:${port}`,
        body: "Reach it from this Mac while the terminal stays open.",
        label: "Local port",
        initial: String(localPortFor(port)),
        confirmLabel: "Forward",
    });
    if (typed === null) return;
    const local = Number(typed.trim());
    if (!Number.isInteger(local) || local < 1 || local > 65_535) {
        notify("error", "Type a port from 1 to 65535.");
        return;
    }
    notify("info", `${service.name} answers on http://localhost:${local} while the forward runs.`);
    await runInTerminal(`Forward ${service.name}`, [
        "kubectl",
        ...kubectlScope({ context: scope.context, namespace: service.namespace }),
        "port-forward",
        `service/${service.name}`,
        `${local}:${port}`,
    ]);
}

export function Network({
    services,
    ingresses,
    find,
    scope,
    showNamespace,
    onPods,
}: {
    services: Service[];
    ingresses: Ingress[];
    find: string;
    scope: Scope;
    showNamespace: boolean;
    onPods: (service: Service) => void;
}) {
    const words = find.trim().toLowerCase();
    const shownServices = services.filter((service) => !words || service.name.includes(words));
    const shownIngresses = ingresses.filter(
        (ingress) =>
            !words || ingress.name.includes(words) || ingress.rules.some((rule) => rule.host.includes(words) || rule.service.includes(words)),
    );
    return (
        <div className="k8s-network">
            <section className="k8s-section" aria-label="Services">
                <h3 className="k8s-heading">Services</h3>
                {shownServices.length === 0 ? (
                    <p className="k8s-note">{services.length === 0 ? "No services here." : "No services match."}</p>
                ) : (
                    <table className="k8s-table" aria-label="Services">
                        <thead>
                            <tr>
                                <th>Name</th>
                                <th>Type</th>
                                <th>Cluster IP</th>
                                <th>Ports</th>
                                <th>Picks pods</th>
                                <th aria-label="Actions" />
                            </tr>
                        </thead>
                        <tbody>
                            {shownServices.map((service) => {
                                const port = service.ports.map(servicePort).find((each) => each !== null) ?? null;
                                return (
                                    <tr key={`${service.namespace}/${service.name}`} className="k8s-row" aria-label={service.name}>
                                        <td>
                                            <span className="k8s-name">{service.name}</span>
                                            {showNamespace ? <span className="k8s-chip">{service.namespace}</span> : null}
                                            {service.external.length > 0 ? (
                                                <span className="k8s-note-inline k8s-mono">{service.external.join(", ")}</span>
                                            ) : null}
                                        </td>
                                        <td className="k8s-muted">{service.kind}</td>
                                        <td className="k8s-mono k8s-muted">{service.clusterIp ?? "None"}</td>
                                        <td className="k8s-mono">{service.ports.join(", ") || "—"}</td>
                                        <td>
                                            {service.selector ? (
                                                <button type="button" className="k8s-link k8s-mono" onClick={() => onPods(service)}>
                                                    {service.selector}
                                                </button>
                                            ) : (
                                                <span className="k8s-muted">—</span>
                                            )}
                                        </td>
                                        <td className="k8s-cell-actions">
                                            {port !== null && service.kind !== "ExternalName" ? (
                                                <span className="k8s-row-actions">
                                                    <button
                                                        type="button"
                                                        className="k8s-button small"
                                                        aria-label={`Forward ${service.name}`}
                                                        onClick={() => void forward(scope, service, port)}>
                                                        Forward…
                                                    </button>
                                                </span>
                                            ) : null}
                                        </td>
                                    </tr>
                                );
                            })}
                        </tbody>
                    </table>
                )}
            </section>
            <section className="k8s-section" aria-label="Ingresses">
                <h3 className="k8s-heading">Ingresses</h3>
                {shownIngresses.length === 0 ? (
                    <p className="k8s-note">{ingresses.length === 0 ? "Nothing routes in from outside the cluster." : "No ingresses match."}</p>
                ) : (
                    shownIngresses.map((ingress) => (
                        <article key={`${ingress.namespace}/${ingress.name}`} className="k8s-ingress" aria-label={`Ingress ${ingress.name}`}>
                            <header>
                                <span className="k8s-name">{ingress.name}</span>
                                {showNamespace ? <span className="k8s-chip">{ingress.namespace}</span> : null}
                                {ingress.class ? <span className="k8s-chip">{ingress.class}</span> : null}
                                {ingress.address.length > 0 ? <span className="k8s-muted k8s-mono">{ingress.address.join(", ")}</span> : null}
                            </header>
                            <ul className="k8s-routes">
                                {ingress.rules.map((route) => {
                                    const url = routeUrl(route, ingress.tls);
                                    return (
                                        <li key={`${route.host}${route.path}`}>
                                            {url ? (
                                                <button
                                                    type="button"
                                                    className="k8s-link k8s-mono"
                                                    title={`Open ${url}`}
                                                    onClick={() => void openUrl(url)}>
                                                    {route.host}
                                                    {route.path}
                                                </button>
                                            ) : (
                                                <span className="k8s-mono">
                                                    {route.host}
                                                    {route.path}
                                                </span>
                                            )}
                                            {ingress.tls.includes(route.host) ? <span className="k8s-chip">TLS</span> : null}
                                            <span className="k8s-muted">→</span>
                                            <span className="k8s-mono">
                                                {route.service}:{route.port}
                                            </span>
                                        </li>
                                    );
                                })}
                            </ul>
                        </article>
                    ))
                )}
            </section>
        </div>
    );
}
