import { type Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { languageFromPath, lsp, uriToPath, type LspLocation, type LspLocationKind } from "../api/lsp";
import { openLspPeek } from "./lspPeek";
import { notify } from "../state/toast";
import { errorText } from "../lib/format";

export interface LspContext {
    project: string;
    path: string;
    navigate: (path: string, line: number, character: number) => void;
}

const contexts = new WeakMap<EditorView, LspContext>();

type TargetKind = Exclude<LspLocationKind, "declaration" | "typeDefinition">;

const MAX_PEEK_ITEMS = 300;

const TITLE: Record<LspLocationKind, string> = {
    definition: "Definitions",
    declaration: "Declarations",
    typeDefinition: "Type definitions",
    implementation: "Implementations",
    references: "References",
};

export function setLspContext(view: EditorView | null, ctx: LspContext | null) {
    if (!view) return;
    if (ctx) contexts.set(view, ctx);
    else contexts.delete(view);
}

function symbolPositionAt(view: EditorView, pos: number): number | null {
    const line = view.state.doc.lineAt(pos);
    const candidates = [pos, pos > line.from ? pos - 1 : null, pos < line.to ? pos + 1 : null];
    for (const candidate of candidates) {
        if (candidate == null) continue;
        const word = view.state.wordAt(candidate);
        if (!word) continue;
        // LSP servers answer most reliably from inside the token. Raw mouse
        // coords often land on the right edge of a glyph, which used to send
        // gopls a position just outside the identifier.
        return Math.max(word.from, Math.min(candidate, word.to - 1));
    }
    return null;
}

function uniqueLocations(locs: LspLocation[]): LspLocation[] {
    const seen = new Set<string>();
    const out: LspLocation[] = [];
    for (const loc of locs) {
        const key = `${loc.uri}:${loc.range.start.line}:${loc.range.start.character}`;
        if (seen.has(key)) continue;
        seen.add(key);
        out.push(loc);
    }
    return out;
}

async function locationsFor(
    ctx: LspContext,
    lang: string,
    line: number,
    character: number,
    requested: TargetKind,
): Promise<{ kind: LspLocationKind; locs: LspLocation[] }> {
    if (requested === "references") {
        return { kind: "references", locs: uniqueLocations(await lsp.references(ctx.project, lang, ctx.path, line, character)) };
    }
    if (requested === "implementation") {
        return { kind: "implementation", locs: uniqueLocations(await lsp.implementation(ctx.project, lang, ctx.path, line, character)) };
    }

    // VSCode-ish fallback chain. gopls can return nothing for definition on
    // some identifiers where declaration/typeDefinition/implementation is the
    // useful jump target.
    const chain: LspLocationKind[] = ["definition", "declaration", "typeDefinition", "implementation"];
    for (const kind of chain) {
        const fn =
            kind === "definition"
                ? lsp.definition
                : kind === "declaration"
                  ? lsp.declaration
                  : kind === "typeDefinition"
                    ? lsp.typeDefinition
                    : lsp.implementation;
        const locs = uniqueLocations(await fn(ctx.project, lang, ctx.path, line, character));
        if (locs.length > 0) return { kind, locs };
    }
    return { kind: "definition", locs: [] };
}

export function lspNav(): Extension {
    return EditorView.domEventHandlers({
        mousedown(event, view) {
            if (!(event.metaKey || event.ctrlKey)) return false;
            if (event.button !== 0) return false;
            const ctx = contexts.get(view);
            if (!ctx) return false;
            const lang = languageFromPath(ctx.path);
            if (!lang) return false;
            const rawPos = view.posAtCoords({ x: event.clientX, y: event.clientY });
            if (rawPos == null) return false;
            const pos = symbolPositionAt(view, rawPos);
            if (pos == null) return false;
            const line = view.state.doc.lineAt(pos);
            const character = pos - line.from;
            event.preventDefault();

            const requested: TargetKind = event.shiftKey ? "references" : event.altKey ? "implementation" : "definition";

            void locationsFor(ctx, lang, line.number - 1, character, requested)
                .then(({ kind, locs }) => {
                    const latest = contexts.get(view);
                    if (!latest || latest.project !== ctx.project || latest.path !== ctx.path) return;
                    if (locs.length === 0) {
                        notify("info", `No ${requested === "definition" ? "definition" : requested} found`);
                        return;
                    }
                    if (kind !== "references" && locs.length === 1) {
                        const t = locs[0];
                        ctx.navigate(uriToPath(t.uri), t.range.start.line, t.range.start.character);
                        return;
                    }
                    const shown = locs.slice(0, MAX_PEEK_ITEMS);
                    openLspPeek(view, {
                        atLine: line.number - 1,
                        title: locs.length > shown.length ? `${TITLE[kind]} · first ${shown.length}/${locs.length}` : TITLE[kind],
                        items: shown.map((l) => ({
                            uri: l.uri,
                            line: l.range.start.line,
                            character: l.range.start.character,
                        })),
                    });
                })
                .catch((e) => notify("error", `LSP ${requested}: ${errorText(e)}`));
            return true;
        },
    });
}
