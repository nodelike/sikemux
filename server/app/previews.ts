import type { Plugin } from "vite";

interface Preview {
  title: string;
  description: string;
  /** What the preview itself says, when it is not the page's own description. */
  short?: string;
  /** Left out where one built page answers for many addresses, so each link stands for itself. */
  url?: string;
  picture: { src: string; width: number; height: number; alt: string };
  card: "summary" | "summary_large_image";
}

const ICON: Preview["picture"] = {
  src: "https://app.sikemux.com/apple-touch-icon.png",
  width: 256,
  height: 256,
  alt: "Sikemux",
};

/** Each built page and what a link to it previews as. A preview robot never gets the part after `#`, so it can say nothing of what was shared but what its host sent. */
const PREVIEWS: Record<string, Preview> = {
  "index.html": {
    title: "Sign in · Sikemux",
    description:
      "Sign in to your Sikemux account to see the hosts and clients on it. Own your software factory: agents build and ship, you make the calls.",
    short:
      "One account for every device you use with Sikemux. Sign in on each, and they find each other.",
    url: "https://app.sikemux.com/",
    picture: {
      src: "https://sikemux.com/og.png",
      width: 1200,
      height: 630,
      alt: "Sikemux: own your software factory. Agents build and ship, you make the calls.",
    },
    card: "summary_large_image",
  },
  "shared-artifact.html": {
    title: "Shared artifact · Sikemux",
    description:
      "An artifact an agent made, shared from Sikemux. Open the link to see it.",
    picture: ICON,
    card: "summary",
  },
  // What the API serves for a share whose host sent its title, with the places filled in.
  "titled-artifact.html": {
    title: "__TITLE__",
    description: "__ABOUT__",
    picture: {
      src: "https://app.sikemux.com/p/__ID__/preview.png",
      width: 1200,
      height: 630,
      alt: "__TITLE__",
    },
    card: "summary_large_image",
  },
  "shared-chat.html": {
    title: "Shared chat · Sikemux",
    description:
      "A chat with a coding agent, shared from Sikemux. Open the link to read it.",
    picture: ICON,
    card: "summary",
  },
};

const SLOT = "<!--preview-->";

function pageAt(path: string): string {
  if (path.startsWith("/p/")) return "shared-artifact.html";
  if (path.startsWith("/s/")) return "shared-chat.html";
  return "index.html";
}

function tags({
  title,
  description,
  short = description,
  url,
  picture,
  card,
}: Preview): string {
  const meta = (key: "name" | "property", name: string, content: string) =>
    `<meta ${key}="${name}" content="${content}" />`;
  return [
    `<title>${title}</title>`,
    meta("name", "description", description),
    ...(url
      ? [
          `<link rel="canonical" href="${url}" />`,
          meta("property", "og:url", url),
        ]
      : []),
    meta("property", "og:type", "website"),
    meta("property", "og:site_name", "Sikemux"),
    meta("property", "og:title", title),
    meta("property", "og:description", short),
    meta("property", "og:image", picture.src),
    meta("property", "og:image:width", String(picture.width)),
    meta("property", "og:image:height", String(picture.height)),
    meta("property", "og:image:alt", picture.alt),
    meta("property", "og:locale", "en_US"),
    meta("name", "twitter:card", card),
    meta("name", "twitter:title", title),
    meta("name", "twitter:description", short),
    meta("name", "twitter:image", picture.src),
    meta("name", "twitter:creator", "@nodelike"),
  ].join("\n    ");
}

/** Builds the app's one page once for each kind of link, alike but for what the link previews as. Caddy picks which to serve. */
export function previews(): Plugin {
  return {
    name: "sikemux-previews",
    enforce: "post",
    transformIndexHtml(html, { server, originalUrl = "/" }) {
      const preview = server && PREVIEWS[pageAt(originalUrl)];
      return preview ? html.replace(SLOT, tags(preview)) : html;
    },
    generateBundle(_, bundle) {
      const built = bundle["index.html"];
      const html = built?.type === "asset" ? String(built.source) : "";
      if (!built || !html.includes(SLOT))
        this.error("index.html has nowhere to put its preview");
      for (const [fileName, preview] of Object.entries(PREVIEWS)) {
        const source = html.replace(SLOT, tags(preview));
        if (fileName === "index.html") Object.assign(built, { source });
        else this.emitFile({ type: "asset", fileName, source });
      }
    },
  };
}
