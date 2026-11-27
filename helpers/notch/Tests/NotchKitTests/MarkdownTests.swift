import XCTest
@testable import NotchKit

final class MarkdownTests: XCTestCase {
    func testQuestionPartsSingleLineAndHeadingPrefix() {
        let simple = Markdown.questionParts("Ship the new welcome screen?")
        XCTAssertEqual(simple.headline, "Ship the new welcome screen?")
        XCTAssertNil(simple.body)

        let headed = Markdown.questionParts("\n  ## Deploy **v2** to prod?\n\n- Check `canary` metrics\n- Confirm rollback\n")
        XCTAssertEqual(headed.headline, "Deploy **v2** to prod?")
        XCTAssertEqual(headed.body, "- Check `canary` metrics\n- Confirm rollback")
    }

    func testQuestionPartsStartsWithListLeavesHeadlineNil() {
        let parts = Markdown.questionParts("- **First:** item\n- **Second:** item")
        XCTAssertNil(parts.headline)
        XCTAssertEqual(parts.body, "- **First:** item\n- **Second:** item")
    }

    func testQuestionPartsMatchesReportedBenchmarkAsk() {
        let raw = """
        WebArena-Pro benchmark dashboard is ready — open the artifact for the full 1:1 official parity, before/after speedups, per-site breakdown, and failure audit.

        - **Headline:** Sikemux + Antigravity Gemini 4 Neon ranks **#1 on `test39` at 71.8% raw (28/39; 90.0% adjusted)** — **+7.7 pp** over both top official harnesses (`Codex · GPT 6.1 Sol` & `OpenCode · Qwen 3.8 Max Prime`, 64.1%).
        - **Claude Opus 5.5:** Ties official Playwright MCP 1:1 (**61.5%**, 24/39) in 65.1s median wall time, and cut `pilot30` wall time **-33%** (`46.3s → 30.9s`) after our 5 browser fixes.
        - **Reliability:** **0** Sikemux browser-tool bugs across all 138 post-fix runs (and **-66%** tool errors on Agy).
        """
        let parts = Markdown.questionParts(raw)
        XCTAssertEqual(
            parts.headline,
            "WebArena-Pro benchmark dashboard is ready — open the artifact for the full 1:1 official parity, before/after speedups, per-site breakdown, and failure audit."
        )
        let blocks = Markdown.parseBlocks(parts.body ?? "")
        guard case .bulletList(let items) = blocks.first, blocks.count == 1 else {
            return XCTFail("Expected a single bullet list block, got \(blocks)")
        }
        XCTAssertEqual(items.count, 3)

        let firstPlain = Markdown.plainText(items[0].content)
        XCTAssertFalse(firstPlain.contains("**"))
        XCTAssertFalse(firstPlain.contains("`"))
        XCTAssertTrue(firstPlain.contains("Headline: Sikemux + Antigravity Gemini 4 Neon ranks #1 on test39"))
    }

    func testInlineNestedBoldCodeItalicAndLinks() {
        let inlines = Markdown.parseInlines(
            "Ranks **#1 on `test39` raw**, *italic* and _also italic_, ***both***, ~~old~~, [Docs (`v2`)](https://docs.sikemux.com/guides/asks_(v2))."
        )
        XCTAssertEqual(inlines, [
            .text("Ranks "),
            .bold([.text("#1 on "), .code("test39"), .text(" raw")]),
            .text(", "),
            .italic([.text("italic")]),
            .text(" and "),
            .italic([.text("also italic")]),
            .text(", "),
            .boldItalic([.text("both")]),
            .text(", "),
            .strikethrough([.text("old")]),
            .text(", "),
            .link(label: [.text("Docs ("), .code("v2"), .text(")")], url: "https://docs.sikemux.com/guides/asks_(v2)"),
            .text("."),
        ])
    }

    func testIntrawordUnderscoresAndArithmeticStayLiteral() {
        XCTAssertEqual(
            Markdown.parseInlines("Keep foo_bar_baz and 2 * 3 * 4 intact"),
            [.text("Keep foo_bar_baz and 2 * 3 * 4 intact")]
        )
    }

    func testUnclosedDelimitersAndEscapesAreSafe() {
        XCTAssertEqual(
            Markdown.parseInlines(#"Use \*literal\* and **unclosed and `tick"#),
            [.text("Use *literal* and **unclosed and `tick")]
        )
    }

    func testCodeSpansIgnoreFormattingInsideAndSupportMultiBackticks() {
        XCTAssertEqual(
            Markdown.parseInlines("Run `**not bold**` or ``a `b` c``"),
            [.text("Run "), .code("**not bold**"), .text(" or "), .code("a `b` c")]
        )
    }

    func testBlocksHeadingsListsQuotesAndCode() {
        let source = """
        ### Summary
        Intro paragraph
        continues here.
        - First bullet
          - Nested bullet
        * Second bullet

        1. Step one
        2. Step two

        > Quoted note with **emphasis**

        ```bash
        swift test
        ```
        """
        let blocks = Markdown.parseBlocks(source)
        XCTAssertEqual(blocks.count, 6)
        XCTAssertEqual(blocks[0], .heading(level: 3, [.text("Summary")]))
        XCTAssertEqual(blocks[1], .paragraph([.text("Intro paragraph continues here.")]))
        XCTAssertEqual(blocks[2], .bulletList([
            .init(indent: 0, content: [.text("First bullet")]),
            .init(indent: 1, content: [.text("Nested bullet")]),
            .init(indent: 0, content: [.text("Second bullet")]),
        ]))
        XCTAssertEqual(blocks[3], .numberedList([
            .init(indent: 0, number: "1", content: [.text("Step one")]),
            .init(indent: 0, number: "2", content: [.text("Step two")]),
        ]))
        XCTAssertEqual(blocks[4], .quote([.text("Quoted note with "), .bold([.text("emphasis")])]))
        XCTAssertEqual(blocks[5], .codeBlock("swift test"))
    }

    func testAttributedLinksAndSchemeFiltering() {
        let safe = Markdown.attributed(
            Markdown.parseInlines("See [report](https://sikemux.com/r) and [chat](sikemux://artifact/abc)"),
            style: .body
        )
        let links = safe.runs.compactMap(\.link)
        XCTAssertEqual(links, [URL(string: "https://sikemux.com/r")!, URL(string: "sikemux://artifact/abc")!])

        let disallowed = Markdown.attributed(
            Markdown.parseInlines("Ignore [bad](javascript:alert(1))"),
            style: .body
        )
        XCTAssertTrue(disallowed.runs.compactMap(\.link).isEmpty)
        XCTAssertEqual(String(disallowed.characters), "Ignore bad")

        let suppressed = Markdown.attributed(
            Markdown.parseInlines("Choose [this](https://sikemux.com)"),
            style: .optionLabel,
            allowLinks: false
        )
        XCTAssertTrue(suppressed.runs.compactMap(\.link).isEmpty)
        XCTAssertEqual(String(suppressed.characters), "Choose this")
    }

    func testPlainLineStripsMarkdownForCollapsedRows() {
        XCTAssertEqual(
            Markdown.plainLine("**Headline:** Ship `test39` via [dashboard](https://sikemux.com)"),
            "Headline: Ship test39 via dashboard"
        )
        XCTAssertEqual(
            Markdown.plainLine("## **Release** `v2.1`\n\n- Second line ignored"),
            "Release v2.1"
        )
        XCTAssertEqual(
            Markdown.plainLine("- **Item:** check *metrics*"),
            "Item: check metrics"
        )
    }
}
