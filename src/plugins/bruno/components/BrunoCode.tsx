import { useEffect, useRef } from "react";
import {
    Decoration,
    EditorState,
    EditorView,
    RangeSetBuilder,
    StateEffect,
    ViewPlugin,
    auraExtensions,
    basicSetup,
    placeholder as cmPlaceholder,
    registerView,
    html,
    javascript,
    json,
    markdown,
    type DecorationSet,
    type Extension,
    type ViewUpdate,
} from "../../../plugin-api/editor";

import type { Scope } from "../lib/interpolate";

export type BrunoLang = "json" | "javascript" | "xml" | "markdown" | "text";

// Highlight {{variables}} on top of syntax highlighting: teal when resolved in
// the active scope, red when undefined (matches the overlay inputs elsewhere).
const refreshVars = StateEffect.define<null>();

function buildVarDecos(view: EditorView, scope: Scope): DecorationSet {
    const builder = new RangeSetBuilder<Decoration>();
    for (const { from, to } of view.visibleRanges) {
        const text = view.state.doc.sliceString(from, to);
        const re = /\{\{\s*([^}]+?)\s*\}\}/g;
        let m: RegExpExecArray | null;
        while ((m = re.exec(text))) {
            const key = m[1].trim();
            const known = key.startsWith("process.env.") || key in scope;
            builder.add(from + m.index, from + m.index + m[0].length, Decoration.mark({ class: known ? "cm-bruno-var" : "cm-bruno-var missing" }));
        }
    }
    return builder.finish();
}

function varPlugin(scopeRef: { current: Scope }) {
    return ViewPlugin.fromClass(
        class {
            decorations: DecorationSet;
            constructor(view: EditorView) {
                this.decorations = buildVarDecos(view, scopeRef.current);
            }
            update(u: ViewUpdate) {
                if (u.docChanged || u.viewportChanged || u.transactions.some((t) => t.effects.some((e) => e.is(refreshVars)))) {
                    this.decorations = buildVarDecos(u.view, scopeRef.current);
                }
            }
        },
        { decorations: (v) => v.decorations },
    );
}

function langExt(lang: BrunoLang): Extension[] {
    switch (lang) {
        case "json":
            return [json()];
        case "javascript":
            return [javascript()];
        case "markdown":
            return [markdown()];
        case "xml":
            return [html()];
        default:
            return [];
    }
}

/**
 * Reusable CodeMirror surface for the Bruno pane — gives request bodies,
 * scripts, docs and response bodies real syntax highlighting on the shared
 * editor theme. Controlled via `value`; external changes (switching request /
 * tab) are pushed into the doc without disturbing live typing.
 */
export function BrunoCode({
    value,
    lang,
    readOnly = false,
    placeholder,
    className = "",
    onChange,
    vars,
}: {
    value: string;
    lang: BrunoLang;
    readOnly?: boolean;
    placeholder?: string;
    className?: string;
    onChange?: (text: string) => void;
    /** when provided, {{variables}} are highlighted against this scope */
    vars?: Scope;
}) {
    const hostRef = useRef<HTMLDivElement>(null);
    const viewRef = useRef<EditorView | null>(null);
    const lastValue = useRef(value);
    const onChangeRef = useRef(onChange);
    onChangeRef.current = onChange;
    const scopeRef = useRef<Scope>(vars ?? {});
    scopeRef.current = vars ?? {};
    const highlightVars = vars != null;

    useEffect(() => {
        const extensions: Extension[] = [
            basicSetup,
            auraExtensions,
            ...langExt(lang),
            EditorView.lineWrapping,
            EditorView.updateListener.of((u) => {
                if (!u.docChanged) return;
                const text = u.state.doc.toString();
                lastValue.current = text;
                onChangeRef.current?.(text);
            }),
        ];
        if (highlightVars) extensions.push(varPlugin(scopeRef));
        if (placeholder) extensions.push(cmPlaceholder(placeholder));
        if (readOnly) extensions.push(EditorState.readOnly.of(true), EditorView.editable.of(false));

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
        // recreate when language / read-only / placeholder identity changes
    }, [lang, readOnly, placeholder, highlightVars]);

    /*
     * Re-highlight variables when the set of names changes. A highlight only
     * asks whether a name is defined, and the scope object is rebuilt on every
     * keystroke — nudging the editor on each of those made it rebuild every
     * decoration in view for an answer that had not moved.
     */
    const varNames = vars ? Object.keys(vars).sort().join("\u0000") : "";
    useEffect(() => {
        const view = viewRef.current;
        if (view && highlightVars) view.dispatch({ effects: refreshVars.of(null) });
    }, [varNames, highlightVars]);

    // Push external value changes (request or tab switch) into the editor.
    useEffect(() => {
        const view = viewRef.current;
        if (!view || value === lastValue.current) return;
        lastValue.current = value;
        const sel = Math.min(view.state.selection.main.head, value.length);
        view.dispatch({
            changes: { from: 0, to: view.state.doc.length, insert: value },
            selection: { anchor: sel },
        });
    }, [value]);

    return <div ref={hostRef} className={`bruno-cm${className ? ` ${className}` : ""}`} />;
}
