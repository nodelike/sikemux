import { useState } from "react";
import { Seg } from "../../../plugin-api/ui";
import type { Store } from "../api";

type Which = "configMaps" | "secrets";

export function size(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

export function Config({
    configMaps,
    secrets,
    find,
    showNamespace,
}: {
    configMaps: Store[];
    secrets: Store[];
    find: string;
    showNamespace: boolean;
}) {
    const [which, setWhich] = useState<Which>("configMaps");
    const words = find.trim().toLowerCase();
    const keep = (store: Store) => !words || store.name.includes(words) || store.keys.some((key) => key.name.toLowerCase().includes(words));
    const stores = (which === "configMaps" ? configMaps : secrets).filter(keep);
    return (
        <section className="k8s-section" aria-label="Config">
            <div className="k8s-toolbar">
                <Seg
                    label="Kind"
                    value={which}
                    onChange={setWhich}
                    options={[
                        { value: "configMaps", label: "ConfigMaps", count: configMaps.filter(keep).length },
                        { value: "secrets", label: "Secrets", count: secrets.filter(keep).length },
                    ]}
                />
                <span className="k8s-muted k8s-config-note">Only key names are read. Values never leave the cluster.</span>
            </div>
            {stores.length === 0 ? (
                <p className="k8s-note">{words ? "Nothing matches." : which === "secrets" ? "No secrets here." : "No config maps here."}</p>
            ) : (
                <div className="k8s-stores">
                    {stores.map((store) => (
                        <article key={`${store.namespace}/${store.name}`} className="k8s-store" aria-label={store.name}>
                            <header>
                                <span className="k8s-name">{store.name}</span>
                                {showNamespace ? <span className="k8s-chip">{store.namespace}</span> : null}
                                {store.kind && store.kind !== "Opaque" ? <span className="k8s-chip">{store.kind}</span> : null}
                                <span className="k8s-muted">
                                    {store.keys.length} {store.keys.length === 1 ? "key" : "keys"}
                                </span>
                            </header>
                            {store.keys.length > 0 ? (
                                <ul className="k8s-pairs">
                                    {store.keys.map((key) => (
                                        <li key={key.name}>
                                            <span className="k8s-mono">{key.name}</span>
                                            <span className="k8s-muted">{size(key.size)}</span>
                                        </li>
                                    ))}
                                </ul>
                            ) : null}
                        </article>
                    ))}
                </div>
            )}
        </section>
    );
}
