import { StateEffect, StateField, type Extension } from "@codemirror/state";
import { Decoration, EditorView, WidgetType } from "@codemirror/view";
import { fsapi } from "../api/fs";
import { uriToPath } from "../api/lsp";
import { basename } from "@shared/lib/paths";
import { requestOpenFile } from "../state/commands";
import { swallow } from "../state/toast";

export interface PeekItem {
    uri: string;
    line: number;
    character: number;
}

export interface PeekState {
    atLine: number; // 0-based line below which to insert the peek panel
    title: string;
    items: PeekItem[];
}

const setPeek = StateEffect.define<PeekState | null>();

class PeekWidget extends WidgetType {
    constructor(public state: PeekState) {
        super();
    }
    eq(other: PeekWidget) {
        return other.state === this.state;
    }
    toDOM(view: EditorView): HTMLElement {
        const wrap = document.createElement("div");
        wrap.className = "cm-peek";

        const head = document.createElement("div");
        head.className = "cm-peek-head";
        const title = document.createElement("span");
        title.className = "cm-peek-title";
        title.textContent = this.state.title;
        const count = document.createElement("span");
        count.className = "cm-peek-count";
        count.textContent = String(this.state.items.length);
        head.appendChild(title);
        head.appendChild(count);
        const close = document.createElement("button");
        close.className = "cm-peek-close";
        close.textContent = "✕";
        close.title = "Close — esc";
        close.onclick = () => view.dispatch({ effects: setPeek.of(null) });
        head.appendChild(close);
        wrap.appendChild(head);

        const list = document.createElement("div");
        list.className = "cm-peek-list";
        wrap.appendChild(list);

        const byPath = new Map<string, PeekItem[]>();
        for (const it of this.state.items) {
            const p = uriToPath(it.uri);
            const arr = byPath.get(p);
            if (arr) arr.push(it);
            else byPath.set(p, [it]);
        }

        for (const [path, items] of byPath) {
            const group = document.createElement("div");
            group.className = "cm-peek-group";

            const gh = document.createElement("div");
            gh.className = "cm-peek-group-head";
            gh.title = path;
            const gName = document.createElement("span");
            gName.className = "cm-peek-group-name";
            gName.textContent = basename(path);
            const gPath = document.createElement("span");
            gPath.className = "cm-peek-group-path";
            gPath.textContent = path;
            gh.appendChild(gName);
            gh.appendChild(gPath);
            group.appendChild(gh);

            const rows: { row: HTMLButtonElement; txt: HTMLSpanElement; line: number }[] = [];
            for (const it of items) {
                const row = document.createElement("button");
                row.className = "cm-peek-row";
                const ln = document.createElement("span");
                ln.className = "cm-peek-row-line";
                ln.textContent = String(it.line + 1);
                const txt = document.createElement("span");
                txt.className = "cm-peek-row-text";
                txt.textContent = "…";
                row.appendChild(ln);
                row.appendChild(txt);
                row.onclick = () => {
                    requestOpenFile(path, it.line, it.character);
                    view.dispatch({ effects: setPeek.of(null) });
                };
                group.appendChild(row);
                rows.push({ row, txt, line: it.line });
            }

            list.appendChild(group);

            void fsapi
                .readFile(path)
                .then((content) => {
                    const lines = content.split("\n");
                    for (const { txt, line } of rows) {
                        txt.textContent = (lines[line] ?? "").trim();
                    }
                })
                .catch(swallow("lsp peek read"));
        }

        return wrap;
    }
    ignoreEvent() {
        return false;
    }
}

const peekField = StateField.define<PeekState | null>({
    create: () => null,
    update(value, tr) {
        for (const e of tr.effects) if (e.is(setPeek)) value = e.value;
        return value;
    },
    provide: (f) =>
        EditorView.decorations.compute([f], (state) => {
            const v = state.field(f);
            if (!v) return Decoration.none;
            const doc = state.doc;
            if (doc.lines === 0) return Decoration.none;
            const ln = Math.max(0, Math.min(v.atLine, doc.lines - 1));
            const line = doc.line(ln + 1);
            return Decoration.set([
                Decoration.widget({
                    widget: new PeekWidget(v),
                    block: true,
                    side: 1,
                }).range(line.to),
            ]);
        }),
});

const escHandler = EditorView.domEventHandlers({
    keydown(e, view) {
        if (e.key === "Escape" && view.state.field(peekField, false)) {
            view.dispatch({ effects: setPeek.of(null) });
            e.preventDefault();
            return true;
        }
        return false;
    },
});

export function lspPeek(): Extension {
    return [peekField, escHandler];
}

export function openLspPeek(view: EditorView, state: PeekState) {
    view.dispatch({ effects: setPeek.of(state) });
}
