import { useEffect, useRef } from "react";
import { history, defaultKeymap, historyKeymap } from "@codemirror/commands";
import { unifiedMergeView } from "@codemirror/merge";
import { EditorState, Prec } from "@codemirror/state";
import { EditorView, keymap, lineNumbers } from "@codemirror/view";
import { editorThemeOnlyExtensions, languageCompartment, languageFor, loadLanguage } from "../editor/codemirror";
import { registerView } from "../themes/bus";
import { swallow } from "../state/toast";

/* The editor's own theme sets a 13px face and a solid ground; the diff pane
   reads at the diff's size over whatever is behind it. */
const diffSurface = Prec.highest(
    EditorView.theme({
        "&": { backgroundColor: "transparent", fontSize: "12px" },
        ".cm-scroller": { fontFamily: "var(--mono)", lineHeight: "19px" },
        ".cm-content": { fontFamily: "var(--mono)", fontSize: "12px", padding: "8px 0" },
        ".cm-gutters": { backgroundColor: "transparent", color: "var(--ink-faint)", border: "none" },
    }),
);

/* The default gives up on anything past a few hundred changed characters and
   marks the rest of the file as rewritten. The time limit keeps a real
   rewrite from stalling typing. */
const DIFF_CONFIG = { scanLimit: 20_000, timeout: 250 };

/** The working-tree file, editable, with what changed since `base` marked on it. */
export default function DiffMergeEditor({
    base,
    head,
    path,
    tinted,
    onChange,
}: {
    base: string;
    head: string;
    path: string;
    tinted: boolean;
    onChange: (text: string) => void;
}) {
    const host = useRef<HTMLDivElement>(null);
    const view = useRef<EditorView | null>(null);
    const latest = useRef({ head, onChange });
    latest.current = { head, onChange };

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
        const editor = view.current;
        if (!editor || editor.state.doc.toString() === head) return;
        editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: head } });
    }, [head]);

    return <div ref={host} className={`diff-merge-editor${tinted ? " tinted" : ""}`} />;
}
