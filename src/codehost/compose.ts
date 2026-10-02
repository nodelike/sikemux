import type { Pull } from "./api";

const USUAL_BASES = ["main", "master", "develop", "trunk"];

export function isUsualBase(branch: string): boolean {
    return USUAL_BASES.includes(branch);
}

export function defaultBase(branches: readonly string[], head: string | null): string | null {
    const others = branches.filter((branch) => branch !== head);
    return USUAL_BASES.find((name) => others.includes(name)) ?? others[0] ?? null;
}

export function needsPull(branch: string | null, pulls: readonly Pull[], bases: readonly string[]): boolean {
    if (!branch || isUsualBase(branch) || bases.includes(branch)) return false;
    return !pulls.some((pull) => pull.head === branch && pull.state === "open");
}
