const OPENABLE = new Set(["http:", "https:", "mailto:"]);

/** Opens a web or mail link in a new tab; anything else names something on the sharer's computer. */
export function openUrl(href: string): Promise<void> {
  const url = URL.parse(href);
  if (!url || !OPENABLE.has(url.protocol))
    return Promise.reject(new Error(`${href} cannot open here`));
  window.open(url.href, "_blank", "noopener,noreferrer");
  return Promise.resolve();
}
