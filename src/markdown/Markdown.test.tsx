import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { MarkdownBlocks, type MarkdownComponents } from "./Markdown";
import type { MdElement } from "./types";

afterEach(cleanup);

function html(blocks: MdElement[], components?: MarkdownComponents): string {
    const { container } = render(<MarkdownBlocks blocks={blocks} components={components} />);
    return container.innerHTML;
}

describe("drawing a read message", () => {
    it("draws blocks straight into the parent, so its styles reach them", () => {
        expect(
            html([
                { t: "h", l: 2, c: ["What changed"] },
                { t: "p", c: ["a ", { t: "strong", c: ["b"] }, " ", { t: "em", c: ["c"] }, " ", { t: "del", c: ["d"] }, { t: "br" }, "e"] },
                { t: "hr" },
            ]),
        ).toBe("<h2>What changed</h2><p>a <strong>b</strong> <em>c</em> <del>d</del><br>e</p><hr>");
    });

    it("aligns table columns and drops a missing head", () => {
        expect(
            html([
                {
                    t: "table",
                    align: [null, "right"],
                    head: [["File"], ["Lines"]],
                    rows: [[[{ t: "code", v: "a.rs" }], ["+3"]]],
                },
                { t: "table", align: [null], rows: [[["only"]]] },
            ]),
        ).toBe(
            '<table><thead><tr><th>File</th><th style="text-align: right;">Lines</th></tr></thead>' +
                '<tbody><tr><td><code>a.rs</code></td><td style="text-align: right;">+3</td></tr></tbody></table>' +
                "<table><tbody><tr><td>only</td></tr></tbody></table>",
        );
    });

    it("draws task boxes at the front of a tight item and inside a loose one's paragraph", () => {
        expect(
            html([
                { t: "ul", c: [{ t: "li", checked: true, c: ["done"] }] },
                {
                    t: "ol",
                    start: 3,
                    c: [
                        {
                            t: "li",
                            checked: false,
                            c: [
                                { t: "p", c: ["todo"] },
                                { t: "p", c: ["more"] },
                            ],
                        },
                    ],
                },
            ]),
        ).toBe(
            '<ul class="contains-task-list"><li class="task-list-item"><input readonly="" disabled="" type="checkbox" checked=""> done</li></ul>' +
                '<ol class="contains-task-list" start="3"><li class="task-list-item"><p><input readonly="" disabled="" type="checkbox"> todo</p><p>more</p></li></ol>',
        );
    });

    it("names a fence's language the way code highlighters expect", () => {
        expect(
            html([
                { t: "pre", lang: "rust", v: "fn main() {}\n" },
                { t: "pre", v: "plain\n" },
            ]),
        ).toBe('<pre><code class="language-rust">fn main() {}\n</code></pre><pre><code>plain\n</code></pre>');
    });

    it("numbers footnotes where they are mentioned and where they are written", () => {
        expect(
            html([
                { t: "p", c: ["Backoff", { t: "fnref", n: 1 }] },
                { t: "fndef", n: 1, c: [{ t: "p", c: ["See the RFC."] }] },
            ]),
        ).toBe("<p>Backoff<sup>1</sup></p><p><sup>1</sup> See the RFC.</p>");
    });

    it("lets a surface draw links, fences, code and text its own way, but never text inside a link", () => {
        const components: MarkdownComponents = {
            link: ({ href, children }) => <a data-to={href}>{children}</a>,
            fence: ({ lang, text }) => <figure data-lang={lang}>{text}</figure>,
            code: ({ text }) => <kbd>{text}</kbd>,
            text: ({ text }) => <span>{text}</span>,
            heading: ({ level, children }) => (level === 1 ? null : <h6>{children}</h6>),
        };
        expect(
            html(
                [
                    { t: "h", l: 1, c: ["gone"] },
                    { t: "h", l: 3, c: ["kept"] },
                    { t: "p", c: ["see ", { t: "code", v: "x" }, { t: "a", href: "https://a.dev", c: ["in ", { t: "code", v: "y" }] }] },
                    { t: "pre", lang: "ts", v: "z\n" },
                ],
                components,
            ),
        ).toBe(
            "<h6><span>kept</span></h6>" +
                '<p><span>see </span><kbd>x</kbd><a data-to="https://a.dev">in <code>y</code></a></p>' +
                '<figure data-lang="ts">z\n</figure>',
        );
    });

    it("lets a surface draw pictures, and tells it which ones sit inside a link", () => {
        const blocks: MdElement[] = [
            {
                t: "p",
                c: [
                    { t: "img", src: "https://a.dev/1.png", alt: "one" },
                    { t: "a", href: "https://a.dev", c: [{ t: "img", src: "https://a.dev/2.png", alt: "two" }] },
                ],
            },
        ];
        expect(html(blocks)).toBe(
            '<p><img alt="one" src="https://a.dev/1.png"><a href="https://a.dev"><img alt="two" src="https://a.dev/2.png"></a></p>',
        );
        const components: MarkdownComponents = { img: ({ alt, inLink }) => <i data-linked={String(inLink)}>{alt}</i> };
        expect(html(blocks, components)).toBe('<p><i data-linked="false">one</i><a href="https://a.dev"><i data-linked="true">two</i></a></p>');
    });
});
