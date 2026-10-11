import { useEffect, useRef } from "react";
import { EditorState, EditorView, Prec, auraExtensions, basicSetup, keymap, placeholder, registerView } from "../../../plugin-api/editor";
import type { Engine } from "../api";
import { statementAt } from "../sql";
import { sqlHighlighting } from "../sqlHighlight";

/** What ⌘↵ runs: the selection when there is one, otherwise the statement the cursor is in. */
export function sqlUnderCursor(view: EditorView): string {
    const { from, to, head } = view.state.selection.main;
    const text = view.state.doc.toString();
    if (from !== to) return text.slice(from, to);
    return statementAt(text, head)?.text ?? "";
}

export function SqlEditor({
    value,
    dialect,
    onChange,
    onRun,
}: {
    value: string;
    dialect: Engine;
    onChange: (text: string) => void;
    /** Called with the SQL to run: the statement under the cursor, the selection, or everything. */
    onRun: (sql: string) => void;
}) {
    const hostRef = useRef<HTMLDivElement>(null);
    const viewRef = useRef<EditorView | null>(null);
    const lastValue = useRef(value);
    const onChangeRef = useRef(onChange);
    const onRunRef = useRef(onRun);
    onChangeRef.current = onChange;
    onRunRef.current = onRun;

    useEffect(() => {
        const runKeys = Prec.highest(
            keymap.of([
                {
                    key: "Mod-Enter",
                    run: (view) => {
                        onRunRef.current(sqlUnderCursor(view));
                        return true;
                    },
                },
                {
                    key: "Shift-Mod-Enter",
                    run: (view) => {
                        onRunRef.current(view.state.doc.toString());
                        return true;
                    },
                },
            ]),
        );
        const extensions = [
            runKeys,
            basicSetup,
            auraExtensions,
            sqlHighlighting(dialect),
            EditorView.lineWrapping,
            placeholder("select * from …   ⌘↵ runs the statement under the cursor"),
            EditorView.updateListener.of((update) => {
                if (!update.docChanged) return;
                lastValue.current = update.state.doc.toString();
                onChangeRef.current(lastValue.current);
            }),
        ];
        const view = new EditorView({
            parent: hostRef.current!,
            state: EditorState.create({ doc: lastValue.current, extensions }),
        });
        viewRef.current = view;
        const unregister = registerView(view);
        return () => {
            unregister();
            view.destroy();
            viewRef.current = null;
        };
    }, [dialect]);

    useEffect(() => {
        const view = viewRef.current;
        if (!view || value === lastValue.current) return;
        lastValue.current = value;
        view.dispatch({
            changes: { from: 0, to: view.state.doc.length, insert: value },
            selection: { anchor: value.length },
        });
        view.focus();
    }, [value]);

    return <div ref={hostRef} className="db-editor" aria-label="SQL" />;
}
