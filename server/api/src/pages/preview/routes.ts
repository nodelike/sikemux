import type { Context } from "hono";
import { Hono } from "hono";

import type { Services } from "../../app.ts";
import type { Env } from "../../http.ts";
import { ApiFailure, notFound } from "../../http.ts";
import { PAGE_ID } from "../routes.ts";
import { previewedPage, type PreviewedPage } from "../store.ts";

/** Pictures one account's shares may have drawn in a minute. */
const MOST_DRAWN = 10;

const HTML_ENTITIES: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
};
const escapeHtml = (text: string) =>
  text.replace(/[&<>"']/g, (letter) => HTML_ENTITIES[letter] ?? letter);

/** Fills the places `server/app/previews.ts` leaves in the page, all in one pass, so a title cannot name one. */
function fill(template: string, id: string, page: PreviewedPage): string {
  const values: Record<string, string> = {
    __ID__: id,
    __TITLE__: page.title,
    __ABOUT__: page.sharedBy
      ? `Artifact by ${page.sharedBy}, shared from Sikemux.`
      : "An artifact shared from Sikemux.",
  };
  return template.replace(/__(?:ID|TITLE|ABOUT)__/g, (place) =>
    escapeHtml(values[place] ?? ""),
  );
}

/**
 * What a share link previews as. Caddy asks for it at /p/<id> and /p/<id>/preview.png, and answers
 * as it does for an id that was never shared when this says anything but 200.
 */
export function previewRoutes({
  database: { db },
  limiter,
  previews,
}: Pick<Services, "database" | "limiter" | "previews">) {
  const none = () =>
    notFound("No page with a title is shared under that link.");
  const shown = async (c: Context) => {
    const id = c.req.param("id") ?? "";
    const page =
      previews && PAGE_ID.test(id) ? await previewedPage(db, id) : null;
    if (!previews || !page) throw none();
    return { id, page, previews };
  };

  return new Hono<Env>()
    .get("/:id", async (c) => {
      const { id, page, previews } = await shown(c);
      return c.html(fill(previews.template, id, page));
    })
    .get("/:id/preview.png", async (c) => {
      const { id, page, previews } = await shown(c);
      const png = await previews.drawer.picture(
        { id, title: page.title, sharedBy: page.sharedBy },
        () => limiter.allow(`page-picture:${page.userId}`, MOST_DRAWN),
      );
      if (!png)
        throw new ApiFailure(
          503,
          "unavailable",
          "Too many pictures are being drawn; try again in a minute.",
        );
      return c.body(new Uint8Array(png), 200, { "content-type": "image/png" });
    });
}
