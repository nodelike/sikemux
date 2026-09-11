import { RangeSet, RangeSetBuilder, StateEffect, StateField, type Extension } from "@codemirror/state";
import { EditorView, gutter, GutterMarker, ViewPlugin, type ViewUpdate } from "@codemirror/view";
import { diffApi, type DiffHunk } from "../api/diff";
import { swallow } from "../state/toast";
import { LARGE_DOC_BYTES } from "./codemirror";

const setBaseline = StateEffect.define<string>();
const setHunks = StateEffect.define<DiffHunk[]>();

const baselineField = StateField.define<string>({
    create: () => "",
    update(value, tr) {
        for (const e of tr.effects) if (e.is(setBaseline)) value = e.value;
        return value;
    },
});

const hunksField = StateField.define<DiffHunk[]>({
    create: () => [],
    update(value, tr) {
        for (const e of tr.effects) if (e.is(setHunks)) value = e.value;
        return value;
    },
});

class GitMarker extends GutterMarker {
    constructor(readonly kind: "add" | "mod" | "del") {
        super();
    }
    override eq(other: GutterMarker) {
        return other instanceof GitMarker && other.kind === this.kind;
    }
    override toDOM() {
        const el = document.createElement("div");
        el.className = `cm-git-marker cm-git-${this.kind}`;
        return el;
    }
}

function markersFromHunks(view: EditorView, hunks: DiffHunk[]): RangeSet<GitMarker> {
    if (hunks.length === 0) return RangeSet.empty;
    const doc = view.state.doc;
    const docLines = doc.lines;
    const tuples: Array<[number, GitMarker]> = [];
    for (const h of hunks) {
        if (h.kind === "del") {
            const lineNo = h.start + 1;
            if (lineNo < 1 || lineNo > docLines) continue;
            tuples.push([doc.line(lineNo).from, new GitMarker("del")]);
            continue;
        }
        const startLine = h.start + 1;
        if (startLine < 1 || startLine > docLines) continue;
        const endLine = Math.min(h.end, docLines);
        for (let ln = startLine; ln <= endLine; ln++) {
            tuples.push([doc.line(ln).from, new GitMarker(h.kind)]);
        }
    }
    if (tuples.length === 0) return RangeSet.empty;
    tuples.sort((a, b) => a[0] - b[0]);
    const builder = new RangeSetBuilder<GitMarker>();
    for (const [pos, marker] of tuples) builder.add(pos, pos, marker);
    return builder.finish();
}

function scheduleHunks(view: EditorView) {
    let timer: number | undefined;
    let token = 0;
    const run = () => {
        const baseline = view.state.field(baselineField);
        const current = view.state.doc.toString();
        if (!baseline || baseline.length > LARGE_DOC_BYTES || view.state.doc.length > LARGE_DOC_BYTES) {
            view.dispatch({ effects: setHunks.of([]) });
            return;
        }
        if (current === baseline) {
            view.dispatch({ effects: setHunks.of([]) });
            return;
        }
        const my = ++token;
        diffApi
            .hunks(baseline, current)
            .then((hunks) => {
                if (my !== token) return;
                view.dispatch({ effects: setHunks.of(hunks) });
            })
            .catch(swallow("diff hunks"));
    };
    return {
        schedule: () => {
            token++;
            if (timer) window.clearTimeout(timer);
            timer = window.setTimeout(run, 500);
        },
        destroy: () => {
            token++;
            window.clearTimeout(timer);
        },
    };
}

const gitPlugin = ViewPlugin.fromClass(
    class {
        sched: ReturnType<typeof scheduleHunks>;
        constructor(view: EditorView) {
            this.sched = scheduleHunks(view);
            this.sched.schedule();
        }
        update(u: ViewUpdate) {
            const baseChanged = u.startState.field(baselineField) !== u.state.field(baselineField);
            if (u.docChanged || baseChanged) this.sched.schedule();
        }
        destroy() {
            this.sched.destroy();
        }
    },
);

const gitGutterExt = gutter({
    class: "cm-git-gutter",
    markers: (view) => markersFromHunks(view, view.state.field(hunksField)),
});

const overviewRuler = ViewPlugin.fromClass(
    class {
        dom: HTMLDivElement;
        constructor(view: EditorView) {
            this.dom = document.createElement("div");
            this.dom.className = "cm-git-ruler";
            view.dom.appendChild(this.dom);
            this.render(view);
        }
        update(u: ViewUpdate) {
            const hunksChanged = u.startState.field(hunksField) !== u.state.field(hunksField);
            // The ruler only depends on committed hunk state. Rebuilding DOM on
            // every keystroke/selection-adjacent transaction makes dirty files
            // progressively more expensive as hunk count grows.
            if (hunksChanged) this.render(u.view);
        }
        destroy() {
            this.dom.remove();
        }
        render(view: EditorView) {
            const hunks = view.state.field(hunksField);
            const doc = view.state.doc;
            this.dom.replaceChildren();
            if (hunks.length === 0) return;
            const totalLines = doc.lines;
            for (const h of hunks) {
                const startLine = Math.max(1, Math.min(h.start + 1, totalLines));
                const endLine = h.kind === "del" ? startLine : Math.max(startLine, Math.min(h.end, totalLines));
                const top = ((startLine - 1) / totalLines) * 100;
                const span = Math.max(endLine - startLine + 1, 1);
                const height = h.kind === "del" ? 0 : Math.max((span / totalLines) * 100, 0.4);
                const bar = document.createElement("button");
                bar.type = "button";
                bar.className = `cm-git-ruler-bar ${h.kind}`;
                bar.style.top = `${top}%`;
                bar.style.height = `${height}%`;
                bar.title =
                    h.kind === "del"
                        ? `Deleted before line ${startLine}`
                        : h.kind === "add"
                          ? `Added lines ${startLine}–${endLine}`
                          : `Modified lines ${startLine}–${endLine}`;
                bar.onclick = (e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    const line = view.state.doc.line(startLine);
                    view.dispatch({
                        selection: { anchor: line.from },
                        effects: EditorView.scrollIntoView(line.from, { y: "center" }),
                    });
                    view.focus();
                };
                this.dom.appendChild(bar);
            }
        }
    },
);

export function gitDiffGutter(): Extension {
    return [baselineField, hunksField, gitPlugin, gitGutterExt, overviewRuler];
}

export function setGitBaseline(view: EditorView, baseline: string) {
    view.dispatch({ effects: setBaseline.of(baseline) });
}
