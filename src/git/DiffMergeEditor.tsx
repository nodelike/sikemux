import { useEffect, useRef } from "react";
import { history, defaultKeymap, historyKeymap } from "@codemirror/commands";
import { getChunks, unifiedMergeView } from "@codemirror/merge";
import { Compartment, EditorState, Facet, Prec } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, WidgetType, keymap, lineNumbers, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import { editorThemeOnlyExtensions, languageCompartment, languageFor, loadLanguage } from "../editor/codemirror";
import { registerView } from "../themes/bus";
import { swallow } from "../state/toast";
import { ownerNotes, type ChangeBlock, type OwnedFile } from "./changeOwners";

/* The editor's own theme sets a 13px face and a solid ground; the diff pane
   reads at the diff's size over whatever is behind it. */
const diffSurface = Prec.highest(
    EditorView.theme({
        "&": { backgroundColor: "transparent", fontSize: "12px" },
        ".cm-scroller": { fontFamily: "var(--mono)", lineHeight: "19px" },
        ".cm-content": { fontFamily: "var(--mono)", fontSize: "12px", padding: "8px 0" },
        ".cm-gutters": { backgroundColor: "var(--surface-gutter)", color: "var(--ink-faint)", border: "none" },
    }),
);

/* The default gives up on anything past a few hundred changed characters and
   marks the rest of the file as rewritten. The time limit keeps a real
   rewrite from stalling typing. */
const DIFF_CONFIG = { scanLimit: 20_000, timeout: 250 };

const ownedFacet = Facet.define<OwnedFile | undefined, OwnedFile | undefined>({ combine: (values) => values[0] });
const ownedCompartment = new Compartment();

class NoteWidget extends WidgetType {
    constructor(readonly text: string) {
        super();
    }
    eq(other: NoteWidget) {
        return other.text === this.text;
    }
    toDOM() {
        const note = document.createElement("span");
        note.className = "diff-note";
        note.textContent = this.text;
        return note;
    }
}

function noteDecorations(state: EditorState): DecorationSet {
    const owned = state.facet(ownedFacet);
    const chunks = getChunks(state)?.chunks;
    if (!owned || !chunks?.length) return Decoration.none;
    const doc = state.doc;
    const blocks: ChangeBlock[] = chunks.map((chunk) => {
        const start = doc.lineAt(chunk.fromB).number;
        if (chunk.toB <= chunk.fromB) return { start, count: 0 };
        return { start, count: doc.lineAt(Math.min(chunk.toB - 1, doc.length)).number - start + 1 };
    });
    const { lines, gaps } = ownerNotes(blocks, owned);
    const notes = new Map(lines);
    for (const [line, note] of gaps) if (!notes.has(line)) notes.set(Math.min(line, doc.lines), note);
    const widgets = [...notes]
        .filter(([line]) => line <= doc.lines)
        .sort(([a], [b]) => a - b)
        .map(([line, note]) => Decoration.widget({ widget: new NoteWidget(note), side: 1 }).range(doc.line(line).to));
    return Decoration.set(widgets);
}

/* Quiet notes after changed lines saying which agent wrote them. */
const ownerNotesPlugin = ViewPlugin.fromClass(
    class {
        decorations: DecorationSet;
        constructor(view: EditorView) {
            this.decorations = noteDecorations(view.state);
        }
        update(update: ViewUpdate) {
            const moved =
                update.docChanged ||
                update.startState.facet(ownedFacet) !== update.state.facet(ownedFacet) ||
                getChunks(update.startState)?.chunks !== getChunks(update.state)?.chunks;
            if (moved) this.decorations = noteDecorations(update.state);
        }
    },
    { decorations: (plugin) => plugin.decorations },
);

/** The working-tree file, editable, with what changed since `base` marked on it. */
export default function DiffMergeEditor({
    base,
    head,
    path,
    tinted,
    owned,
    onChange,
}: {
    base: string;
    head: string;
    path: string;
    tinted: boolean;
    owned?: OwnedFile;
    onChange: (text: string) => void;
}) {
    const host = useRef<HTMLDivElement>(null);
    const view = useRef<EditorView | null>(null);
    const latest = useRef({ head, owned, onChange });
    latest.current = { head, owned, onChange };

    useEffect(() => {
        const parent = host.current;
        if (!parent) return;
        const editor = new EditorView({
            parent,
            state: EditorState.create({
                doc: latest.current.head,
                extensions: [
                    Prec.lowest(lineNumbers()),
                    history(),
                    keymap.of([...defaultKeymap, ...historyKeymap]),
                    editorThemeOnlyExtensions(),
                    diffSurface,
                    languageCompartment.of(languageFor(path)),
                    ownedCompartment.of(ownedFacet.of(latest.current.owned)),
                    ownerNotesPlugin,
                    unifiedMergeView({
                        original: base,
                        mergeControls: false,
                        collapseUnchanged: { margin: 4, minSize: 4 },
                        diffConfig: DIFF_CONFIG,
                    }),
                    EditorView.updateListener.of((update) => {
                        if (update.docChanged) latest.current.onChange(update.state.doc.toString());
                    }),
                ],
            }),
        });
        view.current = editor;
        const unregister = registerView(editor);
        void loadLanguage(path)
            .then((extensions) => {
                if (view.current === editor) editor.dispatch({ effects: languageCompartment.reconfigure(extensions) });
            })
            .catch(swallow("diff editor language"));
        return () => {
            unregister();
            view.current = null;
            editor.destroy();
        };
    }, [base, path]);

    useEffect(() => {
        view.current?.dispatch({ effects: ownedCompartment.reconfigure(ownedFacet.of(owned)) });
    }, [owned]);

    useEffect(() => {
        const editor = view.current;
        if (!editor || editor.state.doc.toString() === head) return;
        editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: head } });
    }, [head]);

    return <div ref={host} className={`diff-merge-editor${tinted ? " tinted" : ""}`} />;
}
