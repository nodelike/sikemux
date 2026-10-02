import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { describe, expect, it } from "vitest";

const entry = readFileSync(join(process.cwd(), "src", "styles.css"), "utf8");

/* The sheets styles.css pulls in are on screen from the first paint. The rest
   arrive with the component that imports them, so a rule only they define does
   not exist until someone has opened that component. */
function inline(dir: string, css: string): string {
    return css.replace(/@import\s+"\.\/([\w/.-]+\.css)";/g, (_, path: string) => {
        const file = join(dir, path);
        return inline(dirname(file), readFileSync(file, "utf8"));
    });
}
const eager = inline(join(process.cwd(), "src"), entry);

describe("shared chrome", () => {
    /* `.settings-btn` is drawn by the AWS sign-in dialog and the editor's empty
       state as well as by settings, and settings.css only loads once someone
       opens the settings pane. Until then the dialog's Cancel and Sign in read
       as bare text — no fill, no border, no accent on the call to action. */
    it("defines the shared button where every pane can see it", () => {
        expect(eager).toMatch(/\n\.settings-btn\s*\{/);
        expect(eager).toMatch(/\n\.settings-btn\.primary\s*\{[^}]*background:\s*var\(--acc\)/);
    });

    /* The button paints itself with the window-faded raised surface. That was a
       local on `.settings-pane`, so it resolved to nothing anywhere else and
       took the button's background with it. */
    it("defines the faded surfaces at the root rather than on one pane", () => {
        expect(eager).toMatch(/:root\s*\{[^}]*--s-raised:/);
    });
});
