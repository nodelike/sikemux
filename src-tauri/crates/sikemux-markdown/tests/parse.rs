#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::string_slice)]

use serde_json::{json, Value};
use sikemux_markdown::{parse, Options};

const CHAT: Options = Options {
    gfm: true,
    html_as_text: false,
    file_links: true,
    html_images: false,
};

fn blocks(text: &str, options: Options) -> Value {
    serde_json::to_value(parse(text, options, 0)).unwrap()
}

fn chat(text: &str) -> Value {
    blocks(text, CHAT)
}

#[test]
fn a_table_keeps_its_alignment_and_pads_short_rows() {
    let text = "| File | Lines | Status |\n| :-- | --: | :-: |\n| `a.rs` | +3 |\n| b.rs | +1 | done | extra |\n| `a \\| b` | 0 | x |";
    assert_eq!(
        chat(text),
        json!([{
            "t": "table",
            "align": ["left", "right", "center"],
            "head": [["File"], ["Lines"], ["Status"]],
            "rows": [
                [[{"t": "code", "v": "a.rs"}], ["+3"], []],
                [["b.rs"], ["+1"], ["done"]],
                [[{"t": "code", "v": "a | b"}], ["0"], ["x"]],
            ],
        }])
    );
}

#[test]
fn a_table_without_column_labels_has_no_head() {
    let text = "|  |  |\n|--|--|\n| a | b |";
    assert_eq!(
        chat(text),
        json!([{"t": "table", "align": [null, null], "rows": [[["a"], ["b"]]]}])
    );
}

#[test]
fn nested_lists_stay_tight_and_loose_as_written() {
    let text = "1. **Quoting.** first\n   - cap it\n   - reset it\n\n     > quoted\n2. second";
    assert_eq!(
        chat(text),
        json!([{"t": "ol", "c": [
            {"t": "li", "c": [
                {"t": "strong", "c": ["Quoting."]},
                " first",
                {"t": "ul", "c": [
                    {"t": "li", "c": [{"t": "p", "c": ["cap it"]}]},
                    {"t": "li", "c": [{"t": "p", "c": ["reset it"]}, {"t": "quote", "c": [{"t": "p", "c": ["quoted"]}]}]},
                ]},
            ]},
            {"t": "li", "c": ["second"]},
        ]}])
    );
}

#[test]
fn task_items_carry_their_state() {
    let text = "- [x] builds on macOS\n- [ ] builds on Linux\n  - [ ] with `--no-default-features`\n- plain";
    assert_eq!(
        chat(text),
        json!([{"t": "ul", "c": [
            {"t": "li", "checked": true, "c": ["builds on macOS"]},
            {"t": "li", "checked": false, "c": ["builds on Linux", {"t": "ul", "c": [
                {"t": "li", "checked": false, "c": ["with ", {"t": "code", "v": "--no-default-features"}]},
            ]}]},
            {"t": "li", "c": ["plain"]},
        ]}])
    );
}

#[test]
fn an_ordered_list_says_where_it_starts_only_when_not_at_one() {
    assert_eq!(
        chat("1. a\n2. b"),
        json!([{"t": "ol", "c": [{"t": "li", "c": ["a"]}, {"t": "li", "c": ["b"]}]}])
    );
    assert_eq!(
        chat("3. c"),
        json!([{"t": "ol", "start": 3, "c": [{"t": "li", "c": ["c"]}]}])
    );
}

#[test]
fn a_fence_names_its_language_by_the_first_word() {
    assert_eq!(
        chat("```src/a.ts title=x\nconst a = 1;\n```"),
        json!([{"t": "pre", "lang": "src/a.ts", "v": "const a = 1;\n"}])
    );
    assert_eq!(chat("```\n```"), json!([{"t": "pre", "v": ""}]));
    assert_eq!(
        chat("    indented\n    code"),
        json!([{"t": "pre", "v": "indented\ncode\n"}])
    );
}

#[test]
fn a_fence_still_being_written_holds_the_rest_of_the_message() {
    let text =
        "Here is the fix:\n\n```rust\nfn main() {\n    println!(\"hi\");\n\n## not a heading";
    assert_eq!(
        chat(text),
        json!([
            {"t": "p", "c": ["Here is the fix:"]},
            {"t": "pre", "lang": "rust", "v": "fn main() {\n    println!(\"hi\");\n\n## not a heading\n"},
        ])
    );
    assert_eq!(
        chat("text\n\n```ts"),
        json!([{"t": "p", "c": ["text"]}, {"t": "pre", "lang": "ts", "v": ""}])
    );
}

#[test]
fn bare_addresses_become_links_without_the_punctuation_after_them() {
    assert_eq!(
        chat("See https://a.dev/x?y=1. Or www.b.dev, (see https://en.wikipedia.org/wiki/Foo_(bar)) and dev@example.com! http://localhost:3000/a"),
        json!([{"t": "p", "c": [
            "See ",
            {"t": "a", "href": "https://a.dev/x?y=1", "c": ["https://a.dev/x?y=1"]},
            ". Or ",
            {"t": "a", "href": "http://www.b.dev", "c": ["www.b.dev"]},
            ", (see ",
            {"t": "a", "href": "https://en.wikipedia.org/wiki/Foo_(bar)", "c": ["https://en.wikipedia.org/wiki/Foo_(bar)"]},
            ") and ",
            {"t": "a", "href": "mailto:dev@example.com", "c": ["dev@example.com"]},
            "! ",
            {"t": "a", "href": "http://localhost:3000/a", "c": ["http://localhost:3000/a"]},
        ]}])
    );
}

#[test]
fn a_quoted_address_is_still_found() {
    assert_eq!(
        chat("\"www.a.dev\""),
        json!([{"t": "p", "c": ["\"", {"t": "a", "href": "http://www.a.dev", "c": ["www.a.dev"]}, "\""]}])
    );
}

#[test]
fn addresses_are_left_alone_where_they_cannot_be_links() {
    assert_eq!(
        chat("xhttps://a.dev a/b@c.dev v1@2.0 `https://a.dev` [https://a.dev](https://b.dev)"),
        json!([{"t": "p", "c": [
            "xhttps://a.dev a/b@c.dev v1@2.0 ",
            {"t": "code", "v": "https://a.dev"},
            " ",
            {"t": "a", "href": "https://b.dev", "c": ["https://a.dev"]},
        ]}])
    );
}

#[test]
fn links_that_could_run_code_are_emptied() {
    assert_eq!(
        chat("[x](javascript:alert(1)) ![p](data:image/png;base64,AA) <mailto:a@b.dev> <c@d.dev>"),
        json!([{"t": "p", "c": [
            {"t": "a", "href": "", "c": ["x"]},
            " ",
            {"t": "img", "src": "", "alt": "p"},
            " ",
            {"t": "a", "href": "mailto:a@b.dev", "c": ["mailto:a@b.dev"]},
            " ",
            {"t": "a", "href": "mailto:c@d.dev", "c": ["c@d.dev"]},
        ]}])
    );
}

#[test]
fn local_file_links_survive_only_in_the_chat() {
    let text = "[shot](file:///Users/me/a.png)";
    assert_eq!(chat(text)[0]["c"][0]["href"], "file:///Users/me/a.png");
    let preview = Options {
        gfm: true,
        ..Options::default()
    };
    assert_eq!(blocks(text, preview)[0]["c"][0]["href"], "");
}

#[test]
fn markup_is_dropped_unless_it_was_typed() {
    let text = "a <b>bold</b> c\n\n<div>\nblock\n</div>";
    assert_eq!(chat(text), json!([{"t": "p", "c": ["a bold c"]}]));
    let typed = Options {
        html_as_text: true,
        ..CHAT
    };
    assert_eq!(
        blocks(text, typed),
        json!([
            {"t": "p", "c": ["a ", "<b>", "bold", "</b>", " c"]},
            {"t": "p", "c": ["<div>\nblock\n</div>"]},
        ])
    );
}

#[test]
fn an_uploaded_picture_survives_when_asked_for() {
    let text = "<img width=\"92\" alt=\"A shot\" src=\"https://github.com/user-attachments/assets/1\" />\r\n\r\nUse theme colours, <img src='https://a.dev/2.png'> here.";
    let prose = Options {
        html_images: true,
        ..CHAT
    };
    assert_eq!(
        blocks(text, prose),
        json!([
            {"t": "p", "c": [{"t": "img", "src": "https://github.com/user-attachments/assets/1", "alt": "A shot"}]},
            {"t": "p", "c": ["Use theme colours, ", {"t": "img", "src": "https://a.dev/2.png", "alt": ""}, " here."]},
        ])
    );
    assert_eq!(
        chat(text),
        json!([{"t": "p", "c": ["Use theme colours,  here."]}])
    );
}

#[test]
fn a_picture_in_markup_keeps_to_safe_addresses() {
    let prose = Options {
        html_images: true,
        ..CHAT
    };
    assert_eq!(
        blocks("<img src=\"javascript:alert(1)\" alt=\"x\">", prose),
        json!([])
    );
}

#[test]
fn typed_markup_is_never_read_as_an_address() {
    let typed = Options {
        html_as_text: true,
        ..CHAT
    };
    assert_eq!(
        blocks("<https://a.dev>", typed),
        json!([{"t": "p", "c": [{"t": "a", "href": "https://a.dev", "c": ["https://a.dev"]}]}])
    );
    assert_eq!(
        blocks("<a href=\"www.a.dev\">", typed),
        json!([{"t": "p", "c": ["<a href=\"www.a.dev\">"]}])
    );
}

#[test]
fn plain_commonmark_has_no_tables_strikes_or_bare_links() {
    let text = "| a |\n|---|\n\n~~x~~ https://a.dev";
    assert_eq!(
        blocks(text, Options::default()),
        json!([
            {"t": "p", "c": ["| a |\n|---|"]},
            {"t": "p", "c": ["~~x~~ https://a.dev"]},
        ])
    );
}

#[test]
fn inline_formatting_and_breaks() {
    assert_eq!(
        chat("*a* **b** ~~c~~ ~d~ `e`  \nf\ng\\\nh"),
        json!([{"t": "p", "c": [
            {"t": "em", "c": ["a"]}, " ",
            {"t": "strong", "c": ["b"]}, " ",
            {"t": "del", "c": ["c"]}, " ",
            {"t": "del", "c": ["d"]}, " ",
            {"t": "code", "v": "e"},
            {"t": "br"},
            "f\ng",
            {"t": "br"},
            "h",
        ]}])
    );
}

#[test]
fn footnotes_are_numbered_as_they_are_first_mentioned() {
    assert_eq!(
        chat("Backoff[^rfc] matters[^x].\n\n[^rfc]: See the RFC."),
        json!([
            {"t": "p", "c": ["Backoff", {"t": "fnref", "n": 1}, " matters[^x]."]},
            {"t": "fndef", "n": 1, "c": [{"t": "p", "c": ["See the RFC."]}]},
        ])
    );
}

#[test]
fn skip_returns_only_the_blocks_after_it() {
    let text = "# a\n\nb\n\n- c";
    assert_eq!(parse(text, CHAT, 2), parse(text, CHAT, 0)[2..].to_vec());
    assert!(parse(text, CHAT, 9).is_empty());
}

fn char_prefixes(text: &str) -> impl Iterator<Item = &str> {
    text.char_indices()
        .map(|(at, _)| &text[..at])
        .chain(std::iter::once(text))
}

/* A message arrives a few characters at a time and only its last two blocks
are read again: the line being written may yet join the block before it, as
a table row or a heading's underline does. Every block before those two
must already be final. */
#[test]
fn every_block_but_the_last_two_is_final_while_a_message_streams() {
    for sample in [
        include_str!("agent-reply.md"),
        include_str!("agent-review.md"),
    ] {
        let whole = parse(sample, CHAT, 0);
        for prefix in char_prefixes(sample) {
            let partial = parse(prefix, CHAT, 0);
            let settled = partial.len().saturating_sub(2);
            assert_eq!(partial[..settled], whole[..settled], "after {:?}", prefix);
        }
    }
}

#[test]
fn a_review_reads_like_github_shows_it() {
    let review = chat(include_str!("agent-review.md"));
    let kinds: Vec<&str> = review
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["t"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["h", "p", "ol", "hr", "p", "ul", "pre", "p", "p", "ul"]
    );
    assert_eq!(review[2]["c"][2]["c"][1]["t"], "table");
    assert_eq!(
        review[6],
        json!({"t": "pre", "v": "ssh: connect to host dev port 22: Connection refused\nretrying in 500ms\n"})
    );
}
