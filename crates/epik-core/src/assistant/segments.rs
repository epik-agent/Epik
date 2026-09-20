//! The pieces of a finished message: runs of Markdown, and the JSON
//! documents fenced between them.
//!
//! The window draws a JSON document as a folding tree, so it needs to
//! know where a message's `json` fences are. Finding them is a parse —
//! CommonMark's, through pulldown-cmark's offsets — so fence syntax is
//! the grammar's: backticks or tildes, any length, indented up to three
//! spaces.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};

use super::json;

/// One piece of a message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Segment {
    /// A run of Markdown, exactly as it appears in the message.
    Markdown(String),
    /// The contents of a JSON fence, without the fence lines.
    Json(String),
}

/// `text` in pieces, in order: each top-level `json` fence whose
/// contents [`json::rows`] accepts, and the Markdown around them, byte
/// for byte.
pub fn segments(text: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    // Where the Markdown run in progress began.
    let mut run = 0;
    let mut depth = 0;
    let mut events = Parser::new(text).into_offset_iter();
    while let Some((event, range)) = events.next() {
        match event {
            Event::Start(tag) if depth == 0 && is_json_fence(&tag) => {
                // The fence's text, up to and including its own end.
                let contents: String = events
                    .by_ref()
                    .map_while(|(event, _)| match event {
                        Event::Text(line) => Some(line.into_string()),
                        _ => None,
                    })
                    .collect();
                if json::is_tree(&contents) {
                    let lines = lines(text, range);
                    if run < lines.start {
                        segments.push(Segment::Markdown(text[run..lines.start].to_owned()));
                    }
                    segments.push(Segment::Json(contents));
                    run = lines.end;
                }
            }
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
    }
    if run < text.len() {
        segments.push(Segment::Markdown(text[run..].to_owned()));
    }
    segments
}

/// The whole lines a fence at `range` takes up: back over the opening
/// fence's indentation, on through its last line's ending.
fn lines(text: &str, range: Range<usize>) -> Range<usize> {
    let start = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
    let end = text[range.end..]
        .find('\n')
        .map_or(text.len(), |at| range.end + at + 1);
    start..end
}

/// Whether `tag` opens a fenced code block whose info string's first
/// token is `json`, in any case. Tokens part at whitespace and commas.
fn is_json_fence(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::CodeBlock(CodeBlockKind::Fenced(info))
            if info
                .split(|c: char| c == ',' || c.is_whitespace())
                .next()
                .is_some_and(|token| token.eq_ignore_ascii_case("json"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn markdown(text: &str) -> Segment {
        Segment::Markdown(text.to_owned())
    }

    fn json(text: &str) -> Segment {
        Segment::Json(text.to_owned())
    }

    #[test]
    fn a_message_without_fences_is_one_markdown_segment() {
        let text = "A paragraph.\n\n- a list\n\nAnother, with `{\"a\": 1}` inline.\n";
        assert_eq!(segments(text), [markdown(text)]);
    }

    #[test]
    fn an_empty_message_has_no_segments() {
        assert_eq!(segments(""), []);
    }

    #[test]
    fn a_json_fence_between_paragraphs_splits_the_message_in_three() {
        let text = "Here it is:\n\n```json\n{\"a\": 1}\n```\n\nThat was it.\n";
        assert_eq!(
            segments(text),
            [
                markdown("Here it is:\n\n"),
                json("{\"a\": 1}\n"),
                markdown("\nThat was it.\n"),
            ]
        );
    }

    #[test]
    fn a_json_fence_that_does_not_parse_stays_markdown() {
        for text in [
            "Before.\n\n```json\n{not json}\n```\n\nAfter.\n",
            "Before.\n\n```json\n42\n```\n\nAfter.\n",
            "Before.\n\n```json\n```\n\nAfter.\n",
        ] {
            assert_eq!(segments(text), [markdown(text)], "{text:?}");
        }
    }

    #[test]
    fn a_fence_in_another_language_stays_markdown() {
        let text = "```rust\nlet a = [1];\n```\n\n```\n{\"a\": 1}\n```\n\n```jsonc\n[1]\n```\n";
        assert_eq!(segments(text), [markdown(text)]);
    }

    #[test]
    fn a_tilde_fence_is_a_fence() {
        let text = "Before.\n\n~~~~json\n[1, 2]\n~~~~\n";
        assert_eq!(segments(text), [markdown("Before.\n\n"), json("[1, 2]\n")]);
    }

    #[test]
    fn the_info_string_is_matched_by_its_first_token_in_any_case() {
        for fence in ["JSON", "Json", "json title=\"x\"", "json,pretty", "  json"] {
            let text = format!("```{fence}\n[1]\n```\n");
            assert_eq!(segments(&text), [json("[1]\n")], "{fence:?}");
        }
    }

    #[test]
    fn an_indented_fence_loses_its_indentation() {
        let text = "Before.\n\n  ```json\n  {\n    \"a\": 1\n  }\n  ```\nAfter.\n";
        assert_eq!(
            segments(text),
            [
                markdown("Before.\n\n"),
                json("{\n  \"a\": 1\n}\n"),
                markdown("After.\n"),
            ]
        );
    }

    #[test]
    fn a_json_fence_nested_in_a_list_or_a_quote_stays_markdown() {
        for text in [
            "- an item:\n\n  ```json\n  {\"a\": 1}\n  ```\n\nAfter.\n",
            "> ```json\n> {\"a\": 1}\n> ```\n\nAfter.\n",
        ] {
            assert_eq!(segments(text), [markdown(text)], "{text:?}");
        }
    }

    #[test]
    fn two_adjacent_json_fences_are_two_segments() {
        let text = "```json\n[1]\n```\n```json\n[2]\n```\n";
        assert_eq!(segments(text), [json("[1]\n"), json("[2]\n")]);
    }

    #[test]
    fn a_blank_line_between_two_fences_is_markdown() {
        let text = "```json\n[1]\n```\n\n```json\n[2]\n```\n";
        assert_eq!(
            segments(text),
            [json("[1]\n"), markdown("\n"), json("[2]\n")]
        );
    }

    #[test]
    fn a_fence_at_the_very_start_and_at_the_very_end() {
        let text = "```json\n[1]\n```\nBetween.\n```json\n[2]\n```";
        assert_eq!(
            segments(text),
            [json("[1]\n"), markdown("Between.\n"), json("[2]\n")]
        );
    }

    #[test]
    fn a_fence_left_open_runs_to_the_end_of_the_message() {
        let text = "Before.\n\n```json\n{\"a\": 1}\n";
        assert_eq!(
            segments(text),
            [markdown("Before.\n\n"), json("{\"a\": 1}\n")]
        );
    }

    #[test]
    fn a_truncated_document_is_still_a_tree() {
        let text = "```json\n{\"a\": 1, \"b\": \"cu\n```\n";
        assert_eq!(segments(text), [json("{\"a\": 1, \"b\": \"cu\n")]);
    }

    /// The pieces, put back: Markdown as it is, a JSON fence wherever
    /// the input next has its contents' first line.
    #[test]
    fn markdown_segments_are_substrings_of_the_input_in_order() {
        let text = "One.\n\n```json\n[1]\n```\n\nTwo, with ```json inline.\n\n~~~JSON\n{\"b\": 2}\n~~~\n\n```rust\nfn main() {}\n```\n\n- ```json\n  [3]\n  ```\n\nThree.";
        let pieces = segments(text);
        assert_eq!(
            pieces
                .iter()
                .filter(|piece| matches!(piece, Segment::Json(_)))
                .count(),
            2
        );
        let mut rest = text;
        for piece in &pieces {
            let (Segment::Markdown(piece) | Segment::Json(piece)) = piece;
            assert!(!piece.is_empty());
            let at = rest.find(piece.as_str()).expect("in input order");
            rest = &rest[at + piece.len()..];
        }
        for pair in pieces.windows(2) {
            assert!(
                !matches!(pair, [Segment::Markdown(_), Segment::Markdown(_)]),
                "adjacent Markdown is one segment"
            );
        }
    }
}
