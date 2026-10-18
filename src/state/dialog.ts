import { create } from "zustand";

/**
 * App-owned confirm/prompt dialogs.
 *
 * `window.confirm` and `window.prompt` render the platform's own sheet, which
 * ignores the theme, the window opacity, and the app font — a jarring break in
 * a desktop shell that otherwise draws every pixel itself. These requests are
 * promise-based so call sites read like the native calls they replace.
 */

export interface ConfirmRequest {
    title: string;
    /** Supporting copy under the title. Newlines become paragraphs. */
    body?: string;
    confirmLabel?: string;
    cancelLabel?: string;
    /** Styles the confirm button as destructive and focuses cancel first. */
    destructive?: boolean;
    /** Shell commands the answer would let run, shown under the body. */
    commands?: { label: string; command: string }[];
}

/** A confirm with a second way to go ahead besides the confirm button. */
export interface ChoiceRequest extends ConfirmRequest {
    otherLabel: string;
}

/** Which way the person went ahead from a choice, or null when they cancelled. */
export type Choice = "confirm" | "other" | null;

export interface PromptRequest {
    title: string;
    body?: string;
    /** Field label above the input. */
    label?: string;
    initial?: string;
    placeholder?: string;
    confirmLabel?: string;
    cancelLabel?: string;
}

type PendingConfirm = ConfirmRequest & { id: number; kind: "confirm"; resolve: (value: boolean) => void };
type PendingChoice = ChoiceRequest & { id: number; kind: "choice"; resolve: (value: Choice) => void };
type PendingPrompt = PromptRequest & { id: number; kind: "prompt"; resolve: (value: string | null) => void };
export type PendingDialog = PendingConfirm | PendingChoice | PendingPrompt;

interface DialogStore {
    /** Head of the queue — the dialog currently on screen. */
    dialog: PendingDialog | null;
    queue: PendingDialog[];
}

export const useDialogs = create<DialogStore>(() => ({ dialog: null, queue: [] }));

let counter = 1;

function enqueue(pending: PendingDialog): void {
    useDialogs.setState((st) => (st.dialog ? { queue: [...st.queue, pending] } : { dialog: pending }));
}

/** Settle the open dialog and promote the next queued one. */
function settle(id: number, apply: (dialog: PendingDialog) => void): void {
    const open = useDialogs.getState().dialog;
    if (!open || open.id !== id) return;
    apply(open);
    useDialogs.setState((st) => ({ dialog: st.queue[0] ?? null, queue: st.queue.slice(1) }));
}

export function confirmDialog(request: ConfirmRequest): Promise<boolean> {
    return new Promise((resolve) => enqueue({ ...request, id: counter++, kind: "confirm", resolve }));
}

export function choiceDialog(request: ChoiceRequest): Promise<Choice> {
    return new Promise((resolve) => enqueue({ ...request, id: counter++, kind: "choice", resolve }));
}

export function promptDialog(request: PromptRequest): Promise<string | null> {
    return new Promise((resolve) => enqueue({ ...request, id: counter++, kind: "prompt", resolve }));
}

/** Accept the open dialog. `value` is required for prompts, ignored for confirms. */
export function acceptDialog(id: number, value?: string): void {
    settle(id, (dialog) => {
        if (dialog.kind === "confirm") dialog.resolve(true);
        else if (dialog.kind === "choice") dialog.resolve("confirm");
        else dialog.resolve(value ?? "");
    });
}

/** Goes ahead with the open choice's other way. */
export function chooseOther(id: number): void {
    settle(id, (dialog) => {
        if (dialog.kind === "choice") dialog.resolve("other");
    });
}

export function dismissDialog(id: number): void {
    settle(id, (dialog) => {
        if (dialog.kind === "confirm") dialog.resolve(false);
        else dialog.resolve(null);
    });
}

/** Cancel everything — used when the app tears the surface down under a dialog. */
export function resetDialogsForTests(): void {
    const { dialog, queue } = useDialogs.getState();
    for (const pending of [dialog, ...queue]) {
        if (!pending) continue;
        if (pending.kind === "confirm") pending.resolve(false);
        else pending.resolve(null);
    }
    useDialogs.setState({ dialog: null, queue: [] });
}
