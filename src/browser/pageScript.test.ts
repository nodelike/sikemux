import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeAll, beforeEach, describe, expect, it } from "vitest";

/* The script an agent's browser tools run inside a tab. jsdom has no layout,
   so each element's box comes from its data-rect="left,top,width,height" and
   the topmost box under a point is the last one in the document. */
const PAGE_SCRIPT = readFileSync(resolve(__dirname, "../../src-tauri/src/browser/page.js"), "utf8");

interface Changes {
    elements?: string;
    removed?: number[];
    textAdded?: string;
    textRemoved?: string;
}

interface Target {
    index?: number;
    text?: string;
    role?: string;
    selector?: string;
}

interface PageApi {
    state(mode?: string, fullText?: boolean): { elements?: string; text?: string; textLength?: number; changes?: Changes | "none" };
    find(query: string, role?: string | null): { matches: number; elements: string; note?: string };
    locate(text: string, role?: string | null): number;
    point(target: number | Target, expectLabel?: string | null): { index: number; label: string; covered: string | null };
    hitAt(x: number, y: number, expectLabel?: string | null): { hit: { tag: string; label: string; index?: number } | null };
    focus(target: Target | null, text: string, replace?: boolean): { replacing: boolean; into: string };
    valueOf(target: Target | null): { value: string | null; valueLength?: number };
    check(condition: Record<string, string>): { met: boolean; failing: string[] };
    scroll(deltaY: number | null, target: Target | null, to: string | null): { revealed?: string; y: number };
    extract(selector?: string | null): { text?: string; parts?: string[]; matches: number };
    showMarks(visible: boolean): { marked?: number[] };
}

const page = () => (window as unknown as { __sikemux: PageApi }).__sikemux;

const boxOf = (element: Element) => {
    const [left, top, width, height] = (element.getAttribute("data-rect") ?? "0,0,0,0").split(",").map(Number);
    return { left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({}) } as DOMRect;
};

const numberOf = (elements: string | undefined, label: string) => {
    const line = (elements ?? "").split("\n").find((item) => item.endsWith(`> ${label}`));
    return Number(/^\[(\d+)\]/.exec(line ?? "")?.[1]);
};

const load = (html: string) => {
    document.body.innerHTML = html;
    delete (window as unknown as Record<string, unknown>).__sikemux;
    delete (window as unknown as Record<string, unknown>).__sikemuxNumbers;
    delete (window as unknown as Record<string, unknown>).__sikemuxRefs;
    delete (window as unknown as Record<string, unknown>).__sikemuxLast;
    (0, eval)(PAGE_SCRIPT);
};

beforeAll(() => {
    Object.defineProperty(HTMLElement.prototype, "innerText", {
        configurable: true,
        get(this: HTMLElement) {
            return this.textContent ?? "";
        },
    });
    HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
        return boxOf(this);
    };
    Element.prototype.scrollIntoView = () => {};
    document.elementFromPoint = (x: number, y: number) => {
        const under = [...document.querySelectorAll("[data-rect]")].filter((element) => {
            if (element.closest("[inert]")) return false;
            const box = boxOf(element);
            return x >= box.left && x < box.right && y >= box.top && y < box.bottom;
        });
        return under.at(-1) ?? document.body;
    };
});

beforeEach(() => load(""));

describe("element numbers", () => {
    it("stay with their element when the list around it re-renders", () => {
        load(`<button data-rect="0,0,80,20">Cardiologist</button>\n<button data-rect="0,30,80,20">Neurologist</button>`);
        const first = page().state("full");
        const neurologist = numberOf(first.elements, "Neurologist");

        const chip = document.createElement("button");
        chip.textContent = "Dermatologist";
        chip.setAttribute("data-rect", "0,60,80,20");
        document.body.prepend(chip);
        const second = page().state("full");

        expect(numberOf(second.elements, "Neurologist")).toBe(neurologist);
        expect(page().point(neurologist).label).toBe("Neurologist");
    });

    it("refuse a number whose element has left the page", () => {
        load(`<button data-rect="0,0,80,20">Save</button>`);
        const save = numberOf(page().state("full").elements, "Save");
        document.body.innerHTML = `<button data-rect="0,0,80,20">Save</button>`;

        expect(() => page().point(save)).toThrow(/no element/);
    });

    it("say when a number was never handed out on this page, as after a navigation", () => {
        load(`<button data-rect="0,0,80,20">Save</button>`);
        page().state("full");

        expect(() => page().point(40)).toThrow(/no element \[40\]; this page has handed out \[0\] to \[0\]/);
        load(`<button data-rect="0,0,80,20">Save</button>`);
        expect(() => page().point(3)).toThrow(/this page has not been read yet/);
    });

    it("name the element that left, so a stale number is told apart from a wrong one", () => {
        load(`<button data-rect="0,0,80,20">Save</button>`);
        const save = numberOf(page().state("full").elements, "Save");
        document.querySelector("button")!.remove();

        expect(() => page().point(save)).toThrow(/no element \[\d+\] \("Save"\): it has left the page/);
    });

    it("refuse a click whose element no longer carries the expected label", () => {
        load(`<button data-rect="0,0,80,20">Neurologist</button>`);
        const index = numberOf(page().state("full").elements, "Neurologist");
        document.querySelector("button")!.textContent = "Nephrologist";

        expect(() => page().point(index, "Neurologist")).toThrow(/not "Neurologist"; nothing was done/);
        expect(page().point(index, "nephro").label).toBe("Nephrologist");
    });
});

describe("page state", () => {
    it("reports only what changed since the last read", () => {
        load(`<p data-rect="0,0,200,20">Inbox</p>\n<button data-rect="0,30,80,20">Refresh</button>\n<button data-rect="0,60,80,20">Archive</button>`);
        const first = page().state("changes");
        expect(first.elements).toContain("Refresh");

        document.querySelectorAll("button")[1].remove();
        const toast = document.createElement("button");
        toast.textContent = "Undo";
        toast.setAttribute("data-rect", "0,90,80,20");
        document.body.append("\n", toast);
        const next = page().state("changes").changes as Changes;

        expect(next.elements).toMatch(/^\[\d+\] <button[^>]*> Undo$/);
        expect(next.removed).toEqual([numberOf(first.elements, "Archive")]);
        expect(next.textAdded).toBe("Undo");
        expect(next.textRemoved).toBe("Archive");
        expect(page().state("changes").changes).toBe("none");
    });

    const longPage = () =>
        load(
            [
                ...Array.from({ length: 10 }, (_, k) => `<button data-rect="0,${-40 * (k + 1)},80,20">Above ${k}</button>`),
                ...Array.from({ length: 10 }, (_, k) => `<button data-rect="0,${30 * k},80,20">Here ${k}</button>`),
                ...Array.from({ length: 500 }, (_, k) => `<button data-rect="0,${768 + 30 * k},80,20">Below ${k}</button>`),
            ].join("\n"),
        );
    const lineCount = (elements: string | undefined) => (elements ? elements.split("\n").length : 0);

    it("lists everything in view but only the offscreen elements nearest it, and counts the rest", () => {
        longPage();
        const full = page().state("full") as { elements?: string; offscreen?: string };

        expect(lineCount(full.elements)).toBe(50);
        expect(full.elements).toContain("> Here 9");
        expect(full.elements).toContain("> Above 9 [offscreen]");
        expect(full.elements).toContain("> Below 29 [offscreen]");
        expect(full.elements).not.toContain("> Below 30 ");
        expect(full.offscreen).toBe("470 more elements are offscreen and not listed (470 below); scroll toward them or use browser_find");
    });

    it("keeps unlisted elements reachable by number and by find", () => {
        longPage();
        page().state("full");

        const found = page().find("Below 300");
        expect(found.matches).toBe(1);
        const index = Number(/^\[(\d+)\]/.exec(found.elements)?.[1]);
        expect(page().point(index).label).toBe("Below 300");
    });

    it("never reports an unlisted element as removed, and lists it once it comes into view", () => {
        longPage();
        const full = page().state("full");
        expect(page().state("changes").changes).toBe("none");

        const buttons = [...document.querySelectorAll("button")];
        buttons.find((button) => button.textContent === "Below 400")!.remove();
        const afterRemoval = page().state("changes").changes as Changes;
        expect(afterRemoval.removed).toBeUndefined();
        expect(afterRemoval.elements).toBeUndefined();

        buttons.find((button) => button.textContent === "Below 200")!.setAttribute("data-rect", "200,0,80,20");
        const scrolled = page().state("changes").changes as Changes;
        expect(scrolled.elements).toMatch(/^\[\d+\] <button[^>]*> Below 200$/);

        buttons.find((button) => button.textContent === "Below 200")!.remove();
        buttons.find((button) => button.textContent === "Here 0")!.remove();
        const gone = page().state("changes").changes as Changes;
        expect(gone.removed).toHaveLength(2);
        expect(gone.removed).toContain(numberOf(full.elements, "Here 0"));
    });

    it("caps new offscreen elements in a change report too", () => {
        longPage();
        page().state("full");

        for (let k = 0; k < 100; k++) {
            const row = document.createElement("button");
            row.textContent = `Loaded ${k}`;
            row.setAttribute("data-rect", `0,${20000 + 30 * k},80,20`);
            document.body.append(row);
        }
        const next = page().state("changes").changes as Changes & { offscreen?: string };

        expect(lineCount(next.elements)).toBe(40);
        expect(next.offscreen).toBe("60 more new elements are offscreen and not listed (60 below); scroll toward them or use browser_find");
    });

    it("caps the text unless the full text is asked for", () => {
        load(`<p data-rect="0,0,200,20">${"word ".repeat(1000)}</p>`);

        const capped = page().state("full");
        expect(capped.text).toHaveLength(2001);
        expect(capped.textLength).toBe(4999);
        expect(page().state("full", true).text).toHaveLength(4999);
    });
});

describe("finding by label", () => {
    beforeEach(() =>
        load(
            `<a href="/save" data-rect="0,0,80,20">Save draft</a>\n<button data-rect="0,30,80,20">Save</button>\n<button data-rect="0,60,80,20" aria-label="Close dialog">×</button>\n<div role="tab" data-rect="0,90,80,20">Save</div>`,
        ),
    );

    it("clicks the one element named that way, by text or accessible name", () => {
        const close = page().locate("close dialog");
        expect(page().point(close).label).toBe("Close dialog");
        expect(page().point(page().locate("Save", "button")).label).toBe("Save");
    });

    it("refuses an ambiguous name and lists the candidates", () => {
        expect(() => page().locate("Save")).toThrow(/2 elements match "Save"; nothing was done[\s\S]*<button[\s\S]*<div>/);
        expect(() => page().locate("Delete")).toThrow(/nothing is labelled "Delete"/);
    });

    it("finds partial matches with their numbers", () => {
        const found = page().find("save");
        expect(found.matches).toBe(2);
        expect(page().find("sav").matches).toBe(3);
        expect(page().find("sav", "link").elements).toMatch(/<a> Save draft/);
    });
});

describe("annotated screenshots", () => {
    it("box only what a person can see", () => {
        load(
            [
                `<main data-rect="0,0,1024,768" tabindex="0">`,
                `<button data-rect="10,10,80,20">Visible</button>`,
                `<button data-rect="10,40,80,20" inert>Inert</button>`,
                `<div aria-hidden="true"><button data-rect="10,70,80,20">Hidden from readers</button></div>`,
                `<button data-rect="-300,100,80,20">Off canvas</button>`,
                `<button data-rect="10,130,80,20">Covered</button>`,
                `</main>`,
                `<div data-rect="0,120,200,60">sticky header</div>`,
            ].join("\n"),
        );
        const elements = page().state("full").elements;
        const marked = page().showMarks(true).marked;

        expect(marked).toEqual([numberOf(elements, "Visible")]);
    });

    it("mark only what sits inside an open modal", () => {
        load(
            [
                `<button data-rect="10,10,80,20">Behind</button>`,
                `<div role="dialog" aria-modal="true" data-rect="200,200,300,200"><button data-rect="220,220,80,20">Confirm</button></div>`,
            ].join("\n"),
        );
        const elements = page().state("full").elements;

        expect(page().showMarks(true).marked).toEqual([numberOf(elements, "Confirm")]);
    });

    it("keep a box larger than most of the view only when it is alone", () => {
        load(`<div tabindex="0" data-rect="0,0,1024,768">Card</div>`);
        page().state("full");
        expect(page().showMarks(true).marked).toHaveLength(1);

        load(`<div tabindex="0" data-rect="0,0,1024,768">Card\n<button data-rect="10,10,80,20">Open</button></div>`);
        const elements = page().state("full").elements;
        expect(page().showMarks(true).marked).toEqual([numberOf(elements, "Open")]);
    });
});

describe("targets", () => {
    it("resolve a CSS selector to one numbered element, and list the candidates when it names several", () => {
        load(
            `<button class="go" data-rect="0,0,80,20">Go</button>\n<button class="stop" data-rect="0,30,80,20">Stop</button>\n<button class="stop" data-rect="0,60,80,20">Halt</button>`,
        );

        expect(page().point({ selector: ".go" }).label).toBe("Go");
        expect(() => page().point({ selector: ".stop" })).toThrow(/2 shown elements match "\.stop"[\s\S]*Stop[\s\S]*Halt/);
        expect(() => page().point({ selector: "##" })).toThrow(/not a valid CSS selector/);
    });

    it("say what a point lands on, and refuse one that is not the expected element", () => {
        load(`<button data-rect="0,0,80,20">Copy</button>\n<p data-rect="0,40,200,20">Paragraph</p>`);
        const elements = page().state("full").elements;

        expect(page().hitAt(10, 10).hit).toEqual({ tag: "button", label: "Copy", index: numberOf(elements, "Copy") });
        expect(() => page().hitAt(10, 45, "Copy")).toThrow(/is on p "Paragraph", not "Copy"; nothing was done/);
        expect(() => page().hitAt(5000, 10)).toThrow(/outside the/);
    });
});

describe("typing", () => {
    it("types over the focused field's text only when asked to replace it", () => {
        load(`<input data-rect="0,0,80,20" value="old text">`);
        const input = document.querySelector("input")!;
        input.focus();

        expect(page().focus(null, "new", false).replacing).toBe(false);
        expect(page().focus(null, "new", true).replacing).toBe(true);
        expect(input.selectionStart).toBe(0);
        expect(input.selectionEnd).toBe(8);
    });

    it("refuses to type when nothing is focused", () => {
        load(`<input data-rect="0,0,80,20">`);
        expect(() => page().focus(null, "x")).toThrow(/nothing is focused, so there is nowhere to type/);
    });

    it("reports the end of a long value with its length, and never a password", () => {
        load(`<textarea data-rect="0,0,80,20"></textarea>\n<input type="password" data-rect="0,30,80,20" value="hunter22">`);
        document.querySelector("textarea")!.value = `${"a".repeat(500)}END`;

        const long = page().valueOf({ selector: "textarea" });
        expect(long.valueLength).toBe(503);
        expect(long.value!.endsWith("aEND")).toBe(true);
        expect(page().valueOf({ selector: "input" }).value).toBe("••••••••");
    });
});

describe("element lines", () => {
    it("show the state a person sees, including a switch that hides its checkbox", () => {
        load(
            [
                `<input name="email" data-rect="0,0,80,20" value="a@b.c">`,
                `<label data-rect="0,30,80,20">Public bot<input type="checkbox" style="opacity:0"></label>`,
                `<button role="switch" aria-checked="true" data-rect="0,60,80,20">Intents</button>`,
                `<button aria-expanded="false" data-rect="0,90,80,20">Menu</button>`,
            ].join("\n"),
        );
        const elements = page().state("full").elements!;

        expect(elements).toMatch(/name=email> a@b\.c/);
        expect(elements).toMatch(/Public bot \[unchecked\]/);
        expect(elements).toMatch(/Intents \[checked\]/);
        expect(elements).toMatch(/Menu \[collapsed\]/);
    });

    it("are found by name, id and the element that labels them", () => {
        load(
            `<span id="caption">Scene prompt</span>\n<textarea id="prompt-input" aria-labelledby="caption" data-rect="0,0,80,20"></textarea>\n<input name="postcode" data-rect="0,30,80,20">`,
        );

        expect(page().find("prompt-input").matches).toBe(1);
        expect(page().find("scene prompt").matches).toBe(1);
        expect(page().find("postcode").matches).toBe(1);
    });

    it("list every element of a role when none of them has the name asked for", () => {
        load(`<textarea data-rect="0,0,80,20" placeholder="Describe the scene"></textarea>`);

        const found = page().find("prompt", "textbox");
        expect(found.matches).toBe(0);
        expect(found.elements).toMatch(/Describe the scene/);
    });
});

describe("waiting and reading", () => {
    it("checks every condition and names those that do not hold", () => {
        load(`<p data-rect="0,0,200,20">Saved</p>\n<div class="spinner" data-rect="0,30,20,20"></div>`);

        expect(page().check({ text: "saved" }).met).toBe(true);
        const pending = page().check({ text: "Saved", selectorGone: ".spinner", textGone: "Loading" });
        expect(pending.met).toBe(false);
        expect(pending.failing).toEqual(['".spinner" is still shown']);
        expect(page().check({ url: "/nowhere" }).failing[0]).toMatch(/does not contain "\/nowhere"/);
    });

    it("brings a target into view when no distance is given", () => {
        load(`<button data-rect="0,0,80,20">Top</button>\n<h2 id="pricing" data-rect="0,2000,80,20">Pricing</h2>`);

        expect(page().scroll(null, { selector: "#pricing" }, null).revealed).toBe("Pricing");
    });

    it("extracts each match of a selector on its own, even one with no text", () => {
        load(
            `<div class="card" data-rect="0,0,80,20">One</div>\n<div class="card" data-rect="0,30,80,20"><img alt="Two"></div>\n<div class="card" data-rect="0,60,80,20"></div>`,
        );

        expect(page().extract(".card").parts).toEqual(["One", "<div> with no text", "<div> with no text"]);
        expect(() => page().extract("#missing")).toThrow(/nothing matches "#missing"/);
    });
});
