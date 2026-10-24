import { create } from "zustand";
import type { BruRequest } from "./lib/types";

/*
 * The parts of a Bruno session that never reach disk: a request's unsaved
 * edits, and the secret environment values typed in this session.
 *
 * They used to live inside `sessions`, so every character typed into a request
 * body wrote to the slice the side rail, the top bar, the lifecycle manager and
 * the persistence loop all watch — and two separate serializers had to
 * remember to strip them again on the way out. Nothing outside the Bruno pane
 * has ever needed to see either one.
 */
interface BrunoRuntimeState {
    /** Requests with unsaved edits, by pane and then by file path. */
    drafts: Record<string, Record<string, BruRequest>>;
    /** Secret environment values, by pane and then by name. */
    secretVars: Record<string, Record<string, string>>;
}

const EMPTY: Record<string, string> = {};
const NO_DRAFTS: Record<string, BruRequest> = {};

export const useBrunoRuntime = create<BrunoRuntimeState>(() => ({ drafts: {}, secretVars: {} }));

/*
 * A draft is written on every keystroke and read when something is about to act
 * on it, so the writes are coalesced and every read flushes first. The window is
 * short enough that a tab strip's dirty dot still looks immediate.
 */
const DRAFT_WRITE_DELAY_MS = 150;

let pendingDraft: { paneId: string; path: string; request: BruRequest | null } | null = null;
let draftTimer: number | null = null;

function commitDraft({ paneId, path, request }: { paneId: string; path: string; request: BruRequest | null }): void {
    useBrunoRuntime.setState((state) => {
        const current = state.drafts[paneId] ?? NO_DRAFTS;
        if (request === null) {
            if (!(path in current)) return state;
            const { [path]: _removed, ...rest } = current;
            return { drafts: { ...state.drafts, [paneId]: rest } };
        }
        if (current[path] === request) return state;
        return { drafts: { ...state.drafts, [paneId]: { ...current, [path]: request } } };
    });
}

/** Write any draft still waiting on its timer. Safe to call when there is none. */
export function flushBrunoDrafts(): void {
    if (draftTimer !== null) {
        window.clearTimeout(draftTimer);
        draftTimer = null;
    }
    const write = pendingDraft;
    pendingDraft = null;
    if (write) commitDraft(write);
}

/** Stash an edited request by file path; pass null to clear the draft. */
export function setBrunoDraft(paneId: string, path: string, request: BruRequest | null): void {
    if (pendingDraft && (pendingDraft.paneId !== paneId || pendingDraft.path !== path)) flushBrunoDrafts();
    pendingDraft = { paneId, path, request };
    if (draftTimer === null) draftTimer = window.setTimeout(flushBrunoDrafts, DRAFT_WRITE_DELAY_MS);
}

export function brunoDrafts(paneId: string): Record<string, BruRequest> {
    flushBrunoDrafts();
    return useBrunoRuntime.getState().drafts[paneId] ?? NO_DRAFTS;
}

export function setBrunoSecret(paneId: string, name: string, value: string): void {
    useBrunoRuntime.setState((state) => {
        const current = state.secretVars[paneId] ?? EMPTY;
        if (current[name] === value) return state;
        return { secretVars: { ...state.secretVars, [paneId]: { ...current, [name]: value } } };
    });
}

export function forgetBrunoPane(paneId: string): void {
    if (pendingDraft?.paneId === paneId) {
        pendingDraft = null;
        if (draftTimer !== null) {
            window.clearTimeout(draftTimer);
            draftTimer = null;
        }
    }
    useBrunoRuntime.setState((state) => {
        if (!(paneId in state.drafts) && !(paneId in state.secretVars)) return state;
        const { [paneId]: _drafts, ...drafts } = state.drafts;
        const { [paneId]: _secrets, ...secretVars } = state.secretVars;
        return { drafts, secretVars };
    });
}

export function useBrunoDrafts(paneId: string): Record<string, BruRequest> {
    return useBrunoRuntime((state) => state.drafts[paneId] ?? NO_DRAFTS);
}

export function useBrunoSecretVars(paneId: string): Record<string, string> {
    return useBrunoRuntime((state) => state.secretVars[paneId] ?? EMPTY);
}
