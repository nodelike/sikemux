import { StreamLanguage, type StreamParser } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import type { Grammar } from "shiki/core";
import type { IToken, StateStack } from "shiki/textmate";
import { textMateGrammar } from "../chat/shikiTokens";

/* Past this a line is minified or generated, and reading it would stall typing. */
const MAX_LINE_LENGTH = 3_000;
/* Shiki's own limit. The first lines read also build the grammar's patterns, which takes most of it. */
const LINE_TIME_LIMIT_MS = 500;

/* How the scope names a grammar gives its tokens map onto the editor's own
   highlighting. The first rule a scope starts with wins. */
const SCOPE_RULES: ReadonlyArray<readonly [string, string]> = [
    ["comment", "comment"],
    ["punctuation.definition.comment", "comment"],
    ["string.regexp", "regexp"],
    ["constant.character.escape", "escape"],
    ["string", "string"],
    ["punctuation.definition.string", "string"],
    ["constant.numeric", "number"],
    ["constant.language.boolean", "bool"],
    ["constant.language.null", "null"],
    ["constant.language.undefined", "null"],
    ["constant.language", "atom"],
    ["constant.other", "variableName.constant"],
    ["variable.other.constant", "variableName.constant"],
    ["keyword.operator", "operator"],
    ["keyword", "keyword"],
    ["storage", "keyword"],
    ["entity.name.function", "variableName.function"],
    ["support.function", "variableName.function"],
    ["entity.name.tag", "tagName"],
    ["entity.other.attribute-name", "attributeName"],
    ["entity.name.type", "typeName"],
    ["entity.name.class", "typeName"],
    ["entity.other.inherited-class", "typeName"],
    ["support.type", "typeName"],
    ["support.class", "typeName"],
    ["entity.name.namespace", "namespace"],
    ["entity.name.section", "heading"],
    ["markup.heading", "heading"],
    ["markup.bold", "strong"],
    ["markup.italic", "emphasis"],
    ["markup.underline.link", "link"],
    ["markup.inserted", "inserted"],
    ["markup.deleted", "deleted"],
    ["variable.other.property", "propertyName"],
    ["variable.other.object.property", "propertyName"],
    ["support.variable.property", "propertyName"],
    ["meta.object-literal.key", "propertyName"],
    ["variable.language", "self"],
    ["variable.parameter", "variableName.definition"],
    ["variable", "variableName"],
    ["punctuation", "punctuation"],
    ["invalid", "invalid"],
];

const styles = new Map<string, string | null>();

function styleOfScope(scope: string): string | null {
    let style = styles.get(scope);
    if (style === undefined) {
        style = SCOPE_RULES.find(([prefix]) => scope === prefix || scope.startsWith(`${prefix}.`))?.[1] ?? null;
        styles.set(scope, style);
    }
    return style;
}

function styleOf(scopes: readonly string[]): string | null {
    for (let i = scopes.length - 1; i >= 0; i--) {
        const style = styleOfScope(scopes[i]);
        if (style) return style;
    }
    return null;
}

interface LineState {
    stack: StateStack | null;
    tokens: IToken[] | null;
    next: number;
}

export function textMateParser(grammar: Grammar): StreamParser<LineState> {
    return {
        startState: () => ({ stack: null, tokens: null, next: 0 }),
        copyState: (state) => ({ ...state }),
        blankLine(state) {
            state.stack = grammar.tokenizeLine("", state.stack, LINE_TIME_LIMIT_MS).ruleStack;
        },
        token(stream, state) {
            if (stream.sol()) {
                if (stream.string.length > MAX_LINE_LENGTH) {
                    state.tokens = null;
                    stream.skipToEnd();
                    return null;
                }
                const line = grammar.tokenizeLine(stream.string, state.stack, LINE_TIME_LIMIT_MS);
                state.stack = line.ruleStack;
                state.tokens = line.tokens;
                state.next = 0;
            }
            const token = state.tokens?.[state.next++];
            if (!token || token.endIndex <= stream.pos) {
                stream.skipToEnd();
                return null;
            }
            stream.pos = Math.min(token.endIndex, stream.string.length);
            return styleOf(token.scopes);
        },
    };
}

/** A grammar the app ships no editor language for, highlighted by the grammar diffs and chat use. */
export async function textMateLanguage(id: string): Promise<Extension[]> {
    const grammar = await textMateGrammar(id);
    return grammar ? [StreamLanguage.define({ name: id, ...textMateParser(grammar) })] : [];
}
