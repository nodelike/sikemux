import { useCallback, useRef } from "react";
import type { EditorView } from "@codemirror/view";

export interface NavEntry {
    path: string;
    line: number;
    character: number;
}

interface NavOptions {
    project: string;
    getView: () => EditorView | null;
    getCurrentPath: () => string | null;
    scrollLiveTo: (line: number, character: number) => void;
    openOther: (entry: NavEntry) => void;
}

const NAV_HISTORY_CAPACITY = 128;

const sameEntry = (left: NavEntry, right: NavEntry): boolean =>
    left.path === right.path && left.line === right.line && left.character === right.character;

/** Browser-style back and forward through the places a jump left from and went to. */
class NavHistory {
    private readonly backward: NavEntry[] = [];
    private readonly forward: NavEntry[] = [];
    private current: NavEntry | null = null;

    push(entry: NavEntry): void {
        if (this.current && sameEntry(this.current, entry)) return;
        if (this.current) this.backward.push(this.current);
        this.current = entry;
        this.forward.length = 0;
        if (this.backward.length >= NAV_HISTORY_CAPACITY) this.backward.shift();
    }

    back(): NavEntry | null {
        return this.move(this.backward, this.forward);
    }

    ahead(): NavEntry | null {
        return this.move(this.forward, this.backward);
    }

    private move(from: NavEntry[], to: NavEntry[]): NavEntry | null {
        const target = from.pop();
        if (!target) return null;
        if (this.current) to.push(this.current);
        this.current = target;
        return target;
    }
}

export function useNavHistory(opts: NavOptions) {
    const optionsRef = useRef(opts);
    optionsRef.current = opts;
    const ownerRef = useRef<{ project: string; history: NavHistory } | null>(null);
    if (!ownerRef.current || ownerRef.current.project !== opts.project) {
        ownerRef.current = { project: opts.project, history: new NavHistory() };
    }

    const captureCurrentPos = useCallback((): NavEntry | null => {
        const current = optionsRef.current;
        const view = current.getView();
        const path = current.getCurrentPath();
        if (!view || !path) return null;
        const head = view.state.selection.main.head;
        const line = view.state.doc.lineAt(head);
        return { path, line: line.number - 1, character: head - line.from };
    }, []);

    const navigateTo = useCallback((entry: NavEntry) => {
        const current = optionsRef.current;
        if (entry.path === current.getCurrentPath() && current.getView()) {
            current.scrollLiveTo(entry.line, entry.character);
        } else {
            current.openOther(entry);
        }
    }, []);

    const push = useCallback(
        (target: NavEntry) => {
            const history = ownerRef.current?.history;
            if (!history) return;
            const origin = captureCurrentPos();
            if (origin) history.push(origin);
            history.push(target);
            navigateTo(target);
        },
        [captureCurrentPos, navigateTo],
    );

    const back = useCallback(() => {
        const target = ownerRef.current?.history.back();
        if (target) navigateTo(target);
    }, [navigateTo]);

    const forward = useCallback(() => {
        const target = ownerRef.current?.history.ahead();
        if (target) navigateTo(target);
    }, [navigateTo]);

    return { push, back, forward };
}
