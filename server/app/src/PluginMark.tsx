import type { ReactNode } from "react";

/** The marks the desktop app draws for its connectors, read from its own files. */
const MARKS = import.meta.glob<string>(
  "../../../app/assets/icons/{bitbucket,cloudflare,database,forgejo,github,gitlab,googleplay,jira,linear,rundeck,signoz,slack,vercel}.svg",
  { query: "?raw", import: "default", eager: true },
);

export function pluginSvg(plugin: string): string | undefined {
  return MARKS[
    `../../../app/assets/icons/${plugin.replace(/^sikemux\./, "")}.svg`
  ];
}

export function PluginMark({
  plugin,
  size,
  fallback,
}: {
  plugin: string;
  size: number;
  fallback: ReactNode;
}) {
  const svg = pluginSvg(plugin);
  if (!svg) return fallback;
  return (
    <span
      className="br"
      style={{ width: size, height: size }}
      aria-hidden="true"
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}
