const RUNTIMES: [RegExp, string][] = [
    [/^(node|next-server|nuxt|vite|npm|pnpm|yarn|bun|tsx|esbuild|webpack|astro|remix)\b/, "nodejs"],
    [/^deno\b/, "deno"],
    [/^(python[\d.]*|uvicorn|gunicorn|hypercorn|flask|django)\b/, "python"],
    [/^(ruby|puma|rails|unicorn)\b/, "ruby"],
    [/^(java|gradle|mvn)\b/, "java"],
    [/^(php|php-fpm)\b/, "php"],
    [/^(beam\.smp|elixir|mix)\b/, "elixir"],
    [/^(com\.docker|docker|vpnkit|orbstack)/, "docker"],
];

/** The file icon of the runtime behind a listening process, or null when its name says nothing about one. */
export function processIcon(process: string): string | null {
    const name = process.trim().toLowerCase().split("/").pop() ?? "";
    for (const [pattern, icon] of RUNTIMES) if (pattern.test(name)) return icon;
    return null;
}
