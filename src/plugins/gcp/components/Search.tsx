import { useEffect, useMemo, useRef, useState } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import type { Target } from "../api";
import { bucketsR, clustersR, functionsR, instancesR, pubsubR, repositoriesR, runJobsR, runServicesR, secretsR, sqlInstancesR } from "../resources";
import { PRODUCTS, type Product } from "../state";
import { ProductIcon } from "./icons";
import { keyOf } from "./parts";

export interface Hit {
    product: Product;
    /** What the product opens it by, as its pages name resources. */
    open: string | null;
    label: string;
    detail: string;
    list?: string;
}

/** Every resource the console has loaded, and the products themselves, matched against words. */
export function search(words: string, hits: Hit[]): Hit[] {
    const needle = words.trim().toLowerCase();
    if (!needle) return [];
    const products: Hit[] = PRODUCTS.map((product) => ({ product: product.id, open: null, label: product.label, detail: "Product" }));
    return [...products, ...hits]
        .filter((hit) => hit.label.toLowerCase().includes(needle) || hit.detail.toLowerCase().includes(needle))
        .sort((a, b) => Number(!a.label.toLowerCase().startsWith(needle)) - Number(!b.label.toLowerCase().startsWith(needle)))
        .slice(0, 12);
}

/** The top bar's search: products and the project's resources, opened where they live. */
export function Search({ active, target, onOpen }: { active: boolean; target: Target | null; onOpen: (hit: Hit) => void }) {
    const [words, setWords] = useState("");
    const [focused, setFocused] = useState(false);
    const [cursor, setCursor] = useState(0);
    const input = useRef<HTMLInputElement>(null);
    const looking = active && !!target && focused && words.trim().length > 0;
    const key = target ? keyOf(target) : (["", ""] as const);
    const run = useResourceEnabled(looking, runServicesR, ...key);
    const jobs = useResourceEnabled(looking, runJobsR, ...key);
    const functions = useResourceEnabled(looking, functionsR, ...key);
    const vms = useResourceEnabled(looking, instancesR, ...key);
    const clusters = useResourceEnabled(looking, clustersR, ...key);
    const buckets = useResourceEnabled(looking, bucketsR, ...key);
    const sql = useResourceEnabled(looking, sqlInstancesR, ...key);
    const pubsub = useResourceEnabled(looking, pubsubR, ...key);
    const secrets = useResourceEnabled(looking, secretsR, ...key);
    const repositories = useResourceEnabled(looking, repositoriesR, ...key);

    const hits = useMemo(
        () =>
            search(words, [
                ...(run.data ?? []).map((each) => ({
                    product: "run" as const,
                    open: `${each.location}/${each.name}`,
                    label: each.name,
                    detail: `Cloud Run service · ${each.location}`,
                    list: "services",
                })),
                ...(jobs.data ?? []).map((each) => ({
                    product: "run" as const,
                    open: `${each.location}/${each.name}`,
                    label: each.name,
                    detail: `Cloud Run job · ${each.location}`,
                    list: "jobs",
                })),
                ...(functions.data ?? []).map((each) => ({
                    product: "functions" as const,
                    open: `${each.location}/${each.name}`,
                    label: each.name,
                    detail: `Function · ${each.location}`,
                })),
                ...(vms.data ?? []).map((each) => ({
                    product: "compute" as const,
                    open: `${each.zone}/${each.name}`,
                    label: each.name,
                    detail: `VM instance · ${each.zone}`,
                })),
                ...(clusters.data ?? []).map((each) => ({
                    product: "gke" as const,
                    open: `${each.location}/${each.name}`,
                    label: each.name,
                    detail: `GKE cluster · ${each.location}`,
                })),
                ...(buckets.data ?? []).map((each) => ({ product: "storage" as const, open: each.name, label: each.name, detail: "Bucket" })),
                ...(sql.data ?? []).map((each) => ({
                    product: "sql" as const,
                    open: each.name,
                    label: each.name,
                    detail: `SQL instance · ${each.region}`,
                })),
                ...(pubsub.data?.topics ?? []).map((each) => ({
                    product: "pubsub" as const,
                    open: each.name,
                    label: each.name,
                    detail: "Pub/Sub topic",
                    list: "topics",
                })),
                ...(pubsub.data?.subscriptions ?? []).map((each) => ({
                    product: "pubsub" as const,
                    open: each.name,
                    label: each.name,
                    detail: "Pub/Sub subscription",
                    list: "subscriptions",
                })),
                ...(secrets.data ?? []).map((each) => ({ product: "secrets" as const, open: each.name, label: each.name, detail: "Secret" })),
                ...(repositories.data ?? []).map((each) => ({
                    product: "registry" as const,
                    open: `${each.location}/${each.name}`,
                    label: each.name,
                    detail: `Repository · ${each.location}`,
                })),
            ]),
        [words, run.data, jobs.data, functions.data, vms.data, clusters.data, buckets.data, sql.data, pubsub.data, secrets.data, repositories.data],
    );

    useEffect(() => {
        const focus = (event: KeyboardEvent) => {
            const typing =
                event.target instanceof HTMLElement &&
                (event.target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(event.target.tagName));
            if (event.key === "/" && !typing && active) {
                event.preventDefault();
                input.current?.focus();
            }
        };
        window.addEventListener("keydown", focus);
        return () => window.removeEventListener("keydown", focus);
    }, [active]);

    const pick = (hit: Hit) => {
        onOpen(hit);
        setWords("");
        input.current?.blur();
    };

    return (
        <div className="gcp-search" role="combobox" aria-expanded={focused && hits.length > 0} aria-haspopup="listbox">
            <svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true">
                <path
                    d="M15.5 14h-.8l-.3-.3A6.5 6.5 0 1 0 14 15.5l.3.3v.8l5 5 1.5-1.5zm-6 0a4.5 4.5 0 1 1 0-9 4.5 4.5 0 0 1 0 9"
                    fill="currentColor"
                />
            </svg>
            <input
                ref={input}
                value={words}
                onChange={(event) => {
                    setWords(event.target.value);
                    setCursor(0);
                }}
                onFocus={() => setFocused(true)}
                onBlur={() => window.setTimeout(() => setFocused(false), 150)}
                onKeyDown={(event) => {
                    if (event.key === "ArrowDown") setCursor((at) => Math.min(at + 1, hits.length - 1));
                    else if (event.key === "ArrowUp") setCursor((at) => Math.max(at - 1, 0));
                    else if (event.key === "Enter" && hits[cursor]) pick(hits[cursor]);
                    else if (event.key === "Escape") {
                        setWords("");
                        input.current?.blur();
                    } else return;
                    event.preventDefault();
                    event.stopPropagation();
                }}
                placeholder="Search (/) for resources and products"
                aria-label="Search Google Cloud"
                aria-controls="gcp-search-results"
                spellCheck={false}
            />
            {focused && hits.length > 0 && (
                <div id="gcp-search-results" className="gcp-search-results" role="listbox" aria-label="Search results">
                    {hits.map((hit, index) => (
                        <button
                            key={`${hit.product}-${hit.detail}-${hit.label}`}
                            type="button"
                            role="option"
                            aria-selected={index === cursor}
                            className={index === cursor ? "on" : undefined}
                            onMouseDown={(event) => event.preventDefault()}
                            onClick={() => pick(hit)}>
                            <ProductIcon product={hit.product} />
                            <span className="gcp-search-label">{hit.label}</span>
                            <span className="gcp-muted">{hit.detail}</span>
                        </button>
                    ))}
                </div>
            )}
        </div>
    );
}
