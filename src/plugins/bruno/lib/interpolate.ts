// {{variable}} interpolation. The scope is a single flat map already merged in
// precedence order by the caller (runtime > environment > folder > collection >
// secrets). Resolution recurses so a var value can reference another var.

export type Scope = Record<string, string>;

/** Merge variable layers; earlier layers win over later ones. */
export function mergeScope(...layers: Scope[]): Scope {
    const out: Scope = {};
    for (let i = layers.length - 1; i >= 0; i--) Object.assign(out, layers[i]);
    return out;
}

export function interpolate(input: string, scope: Scope, depth = 0): string {
    if (!input || !input.includes("{{") || depth > 10) return input;
    const next = input.replace(/\{\{\s*([^}]+?)\s*\}\}/g, (whole, expr: string) => {
        const key = expr.trim();
        if (key.startsWith("process.env.")) return scope[key] ?? "";
        const v = scope[key];
        return v == null ? whole : v;
    });
    return next === input ? next : interpolate(next, scope, depth + 1);
}

/** True if any {{var}} in the string is unresolved against the scope. */
export function hasUnresolved(input: string, scope: Scope): boolean {
    return interpolate(input, scope).includes("{{");
}
