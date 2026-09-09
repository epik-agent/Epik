//! The message grammar: full Markdown, as a pure function into a typed
//! structure.
//!
//! [`parse`] runs pulldown-cmark — CommonMark plus tables,
//! strikethrough, and task lists — and folds the event stream into
//! [`Node`]s, which the chat view maps 1:1 into elements. The structure
//! is the security boundary, by type: there is no raw-HTML node, so a
//! message cannot inject markup — inline and block HTML arrive as
//! ordinary [`Text`], escaped by construction when it lands in a DOM
//! text node. Images never fetch: they collapse to [`Elided`] alt text.
//! Anchors exist only for http(s) destinations — a `javascript:` or
//! `file:` link renders its label as plain text — and bare http(s) URLs
//! in prose autolink, exactly as the old mini-grammar did, but never
//! inside code or an existing link.
//!
//! Deliberately plain here: a ```mermaid fence is an ordinary code
//! block (its info string is kept for a later task to dispatch on),
//! `$x^2$` is ordinary text, and code blocks carry no highlighting.
//!
//! [`Text`]: Node::Text
//! [`Elided`]: Node::Elided

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};

/// One node of a rendered message. What the renderer maps 1:1 into the
/// view, and what the golden tests pin down.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Node {
    /// Plain text, newlines included. Always a DOM text node — escaped
    /// by construction.
    Text(String),
    /// An `inline code` span.
    Code(String),
    Emphasis(Vec<Node>),
    Strong(Vec<Node>),
    Strikethrough(Vec<Node>),
    /// An anchor. Only ever http(s); other schemes never make one.
    Link {
        href: String,
        children: Vec<Node>,
    },
    /// An image that was not fetched: its alt text, elided.
    Elided(String),
    Paragraph(Vec<Node>),
    Heading {
        level: u8,
        children: Vec<Node>,
    },
    /// A fenced or indented code block. `info` is the fence's info
    /// string — unused for styling today, kept for a later task to
    /// dispatch on (mermaid, highlighting).
    CodeBlock {
        info: String,
        code: String,
    },
    BlockQuote(Vec<Node>),
    /// `start` is the first ordinal of an ordered list; `None` is a
    /// bullet list.
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Rule,
    Table {
        header: Vec<Vec<Node>>,
        rows: Vec<Vec<Vec<Node>>>,
    },
}

/// One list item; `checked` is a task-list marker, rendered as a
/// disabled checkbox — nothing in a bubble is interactive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Item {
    pub checked: Option<bool>,
    pub children: Vec<Node>,
}

/// Message text in, structure out. Deterministic, side-effect free, and
/// total: nothing a message says can make it fail.
pub fn parse(text: &str) -> Vec<Node> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut builder = Builder {
        stack: vec![Frame::new(Container::Root)],
    };
    for event in Parser::new_ext(text, options) {
        builder.event(event);
    }
    builder.finish()
}

/// Where the builder currently is; how a frame closes when its End
/// arrives.
enum Container {
    Root,
    Paragraph,
    Heading(u8),
    BlockQuote,
    Emphasis,
    Strong,
    Strikethrough,
    /// `None` for a non-http(s) destination: the label splices into the
    /// parent as plain content.
    Link(Option<String>),
    /// Collects alt text; whatever structure an image caption held is
    /// discarded with the image.
    Image(String),
    CodeBlock(String, String),
    List(Option<u64>, Vec<Item>),
    Item(Option<bool>),
    Table(Vec<Vec<Node>>, Vec<Vec<Vec<Node>>>),
    Row(bool, Vec<Vec<Node>>),
    Cell,
    /// An unmodelled container — an HTML block, a future extension:
    /// its children flow through to the parent.
    Passthrough,
}

struct Frame {
    container: Container,
    nodes: Vec<Node>,
}

impl Frame {
    fn new(container: Container) -> Self {
        Self {
            container,
            nodes: Vec::new(),
        }
    }
}

struct Builder {
    stack: Vec<Frame>,
}

impl Builder {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(_) => self.end(),
            Event::Text(text) => self.text(&text, true),
            Event::Code(code) => {
                if !self.absorb(&code) {
                    self.push(Node::Code(code.into_string()));
                }
            }
            // Raw HTML is never passed through: it lands as literal
            // text, and text nodes cannot be markup.
            Event::Html(html) | Event::InlineHtml(html) => self.text(&html, false),
            Event::SoftBreak | Event::HardBreak => self.text("\n", false),
            Event::Rule => self.push(Node::Rule),
            Event::TaskListMarker(checked) => {
                if let Some(Frame {
                    container: Container::Item(state),
                    ..
                }) = self.stack.last_mut()
                {
                    *state = Some(checked);
                }
            }
            // Math is not enabled and footnotes are not enabled; any
            // other point event a future pulldown adds is not markup.
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        let container = match tag {
            Tag::Paragraph => Container::Paragraph,
            Tag::Heading { level, .. } => Container::Heading(heading_level(level)),
            Tag::BlockQuote(_) => Container::BlockQuote,
            Tag::Emphasis => Container::Emphasis,
            Tag::Strong => Container::Strong,
            Tag::Strikethrough => Container::Strikethrough,
            Tag::Link { dest_url, .. } => Container::Link(http_only(&dest_url)),
            Tag::Image { .. } => Container::Image(String::new()),
            Tag::CodeBlock(kind) => Container::CodeBlock(
                match kind {
                    CodeBlockKind::Fenced(info) => info.into_string(),
                    CodeBlockKind::Indented => String::new(),
                },
                String::new(),
            ),
            Tag::List(start) => Container::List(start, Vec::new()),
            Tag::Item => Container::Item(None),
            Tag::Table(_) => Container::Table(Vec::new(), Vec::new()),
            Tag::TableHead => Container::Row(true, Vec::new()),
            Tag::TableRow => Container::Row(false, Vec::new()),
            Tag::TableCell => Container::Cell,
            _ => Container::Passthrough,
        };
        self.stack.push(Frame::new(container));
    }

    /// Closes the top frame. pulldown balances Start/End, so which tag
    /// ended is settled by what is open.
    fn end(&mut self) {
        if self.stack.len() < 2 {
            return;
        }
        let Frame { container, nodes } = self.stack.pop().expect("checked above");
        match container {
            Container::Root => {}
            Container::Paragraph => self.push(Node::Paragraph(nodes)),
            Container::Heading(level) => self.push(Node::Heading {
                level,
                children: nodes,
            }),
            Container::BlockQuote => self.push(Node::BlockQuote(nodes)),
            Container::Emphasis => self.push(Node::Emphasis(nodes)),
            Container::Strong => self.push(Node::Strong(nodes)),
            Container::Strikethrough => self.push(Node::Strikethrough(nodes)),
            Container::Link(Some(href)) => self.push(Node::Link {
                href,
                children: nodes,
            }),
            // A destination Epik will not anchor: the label stays, the
            // link does not.
            Container::Link(None) | Container::Passthrough => self.splice(nodes),
            Container::Image(alt) => self.push(Node::Elided(alt)),
            Container::CodeBlock(info, code) => self.push(Node::CodeBlock { info, code }),
            Container::List(start, items) => self.push(Node::List { start, items }),
            Container::Item(checked) => {
                if let Some(Frame {
                    container: Container::List(_, items),
                    ..
                }) = self.stack.last_mut()
                {
                    items.push(Item {
                        checked,
                        children: nodes,
                    });
                }
            }
            Container::Table(header, rows) => self.push(Node::Table { header, rows }),
            Container::Row(head, cells) => {
                if let Some(Frame {
                    container: Container::Table(header, rows),
                    ..
                }) = self.stack.last_mut()
                {
                    if head {
                        *header = cells;
                    } else {
                        rows.push(cells);
                    }
                }
            }
            Container::Cell => {
                if let Some(Frame {
                    container: Container::Row(_, cells),
                    ..
                }) = self.stack.last_mut()
                {
                    cells.push(nodes);
                }
            }
        }
    }

    /// Routes source text: into an open code block or image alt when
    /// one is open, otherwise into the flow — autolinked when the text
    /// came from prose and is not already inside a link.
    fn text(&mut self, text: &str, autolink: bool) {
        if self.absorb(text) {
            return;
        }
        let linkable = autolink
            && !self
                .stack
                .iter()
                .any(|frame| matches!(frame.container, Container::Link(_)));
        if linkable {
            let nodes = text_and_links(text);
            self.splice(nodes);
        } else {
            self.push(Node::Text(text.to_owned()));
        }
    }

    /// Absorbs `text` into an open code block or image alt, when one is
    /// open. True when it was taken.
    fn absorb(&mut self, text: &str) -> bool {
        for frame in self.stack.iter_mut().rev() {
            match &mut frame.container {
                Container::CodeBlock(_, code) => {
                    code.push_str(text);
                    return true;
                }
                Container::Image(alt) => {
                    alt.push_str(text);
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    fn push(&mut self, node: Node) {
        if let Some(frame) = self.stack.last_mut() {
            frame.nodes.push(node);
        }
    }

    fn splice(&mut self, nodes: Vec<Node>) {
        if let Some(frame) = self.stack.last_mut() {
            frame.nodes.extend(nodes);
        }
    }

    fn finish(mut self) -> Vec<Node> {
        // A malformed stream cannot leave frames open — pulldown
        // balances its events — but a total function closes them anyway.
        while self.stack.len() > 1 {
            self.end();
        }
        self.stack
            .pop()
            .map(|frame| frame.nodes)
            .unwrap_or_default()
    }
}

const fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// The destinations Epik will anchor: the system browser's protocols
/// and nothing else — never `javascript:`, `file:`, `data:`, ...
fn http_only(url: &str) -> Option<String> {
    (url.starts_with("http://") || url.starts_with("https://")).then(|| url.to_owned())
}

/// Punctuation that ends a sentence rather than a URL.
const TRAILING: &[char] = &['.', ',', ';', ':', '!', '?', ')'];

/// Splits prose into text runs and bare http(s) links — the old
/// mini-grammar's autolink pass, now applied to Markdown text events.
fn text_and_links(text: &str) -> Vec<Node> {
    let mut nodes = Vec::new();
    let mut rest = text;
    loop {
        let start = ["http://", "https://"]
            .iter()
            .filter_map(|scheme| rest.find(scheme))
            .min();
        let Some(start) = start else { break };
        let end = rest[start..]
            .find(char::is_whitespace)
            .map_or(rest.len(), |length| start + length);
        let url = rest[start..end].trim_end_matches(TRAILING);
        if !rest[..start].is_empty() {
            nodes.push(Node::Text(rest[..start].to_owned()));
        }
        nodes.push(Node::Link {
            href: url.to_owned(),
            children: vec![Node::Text(url.to_owned())],
        });
        rest = &rest[start + url.len()..];
    }
    if !rest.is_empty() {
        nodes.push(Node::Text(rest.to_owned()));
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Node {
        Node::Text(s.to_owned())
    }

    fn code(s: &str) -> Node {
        Node::Code(s.to_owned())
    }

    fn link(url: &str) -> Node {
        Node::Link {
            href: url.to_owned(),
            children: vec![text(url)],
        }
    }

    fn paragraph(children: Vec<Node>) -> Node {
        Node::Paragraph(children)
    }

    #[test]
    fn plain_text_is_one_paragraph() {
        assert_eq!(parse("just words"), [paragraph(vec![text("just words")])]);
    }

    #[test]
    fn an_empty_message_is_nothing() {
        assert_eq!(parse(""), []);
    }

    #[test]
    fn a_soft_break_survives_as_a_newline() {
        assert_eq!(
            parse("line one\nline two"),
            [paragraph(vec![
                text("line one"),
                text("\n"),
                text("line two")
            ])]
        );
    }

    // ----- the old mini-grammar's cases, ported -----

    #[test]
    fn a_backtick_span_becomes_code() {
        assert_eq!(
            parse("run `cargo test` now"),
            [paragraph(vec![
                text("run "),
                code("cargo test"),
                text(" now")
            ])]
        );
    }

    #[test]
    fn an_unmatched_backtick_is_just_text() {
        assert_eq!(
            parse("a ` b"),
            [paragraph(vec![text("a "), text("`"), text(" b")])],
            "adjacent text nodes render identically to one"
        );
    }

    #[test]
    fn a_bare_url_becomes_a_link() {
        assert_eq!(
            parse("see https://example.com for more"),
            [paragraph(vec![
                text("see "),
                link("https://example.com"),
                text(" for more"),
            ])]
        );
    }

    #[test]
    fn sentence_punctuation_stays_outside_the_link() {
        assert_eq!(
            parse("read https://example.com/docs."),
            [paragraph(vec![
                text("read "),
                link("https://example.com/docs"),
                text("."),
            ])]
        );
    }

    #[test]
    fn a_parenthesized_url_keeps_its_parenthesis_outside() {
        assert_eq!(
            parse("(https://example.com)"),
            [paragraph(vec![
                text("("),
                link("https://example.com"),
                text(")"),
            ])]
        );
    }

    #[test]
    fn nothing_else_autolinks() {
        assert_eq!(
            parse("ftp://example.com and www.example.com"),
            [paragraph(vec![text(
                "ftp://example.com and www.example.com"
            )])]
        );
    }

    #[test]
    fn a_url_inside_backticks_is_code_not_a_link() {
        assert_eq!(
            parse("`https://example.com`"),
            [paragraph(vec![code("https://example.com")])]
        );
    }

    // ----- markdown links, and the schemes that never anchor -----

    #[test]
    fn a_markdown_link_becomes_an_anchor_with_its_label() {
        assert_eq!(
            parse("[the docs](https://example.com/docs)"),
            [paragraph(vec![Node::Link {
                href: "https://example.com/docs".to_owned(),
                children: vec![text("the docs")],
            }])]
        );
    }

    #[test]
    fn a_javascript_link_is_plain_text_not_an_anchor() {
        assert_eq!(
            parse("[click me](javascript:alert(1))"),
            [paragraph(vec![text("click me")])]
        );
    }

    #[test]
    fn file_and_data_links_are_plain_text_too() {
        assert_eq!(
            parse("[a](file:///etc/passwd) [b](data:text/html,x)"),
            [paragraph(vec![text("a"), text(" "), text("b")])]
        );
    }

    #[test]
    fn a_url_inside_a_link_label_does_not_nest_anchors() {
        assert_eq!(
            parse("[see https://example.com](https://other.com)"),
            [paragraph(vec![Node::Link {
                href: "https://other.com".to_owned(),
                children: vec![text("see https://example.com")],
            }])]
        );
    }

    // ----- images never fetch -----

    #[test]
    fn an_image_is_elided_to_its_alt_text() {
        assert_eq!(
            parse("before ![a diagram](https://example.com/x.png) after"),
            [paragraph(vec![
                text("before "),
                Node::Elided("a diagram".to_owned()),
                text(" after"),
            ])]
        );
    }

    // ----- raw HTML is never passed through -----

    #[test]
    fn a_script_block_arrives_as_literal_text() {
        assert_eq!(
            parse("<script>alert(1)</script>"),
            [text("<script>alert(1)</script>")]
        );
    }

    #[test]
    fn inline_html_arrives_as_literal_text_around_its_words() {
        assert_eq!(
            parse("stay <b>bold</b> word"),
            [paragraph(vec![
                text("stay "),
                text("<b>"),
                text("bold"),
                text("</b>"),
                text(" word"),
            ])]
        );
    }

    // ----- block structure -----

    #[test]
    fn a_heading_keeps_its_level() {
        assert_eq!(
            parse("# Title\n\n### Sub"),
            [
                Node::Heading {
                    level: 1,
                    children: vec![text("Title")],
                },
                Node::Heading {
                    level: 3,
                    children: vec![text("Sub")],
                },
            ]
        );
    }

    #[test]
    fn emphasis_strong_and_strikethrough_nest() {
        assert_eq!(
            parse("*a **b** c* and ~~gone~~"),
            [paragraph(vec![
                Node::Emphasis(vec![text("a "), Node::Strong(vec![text("b")]), text(" c"),]),
                text(" and "),
                Node::Strikethrough(vec![text("gone")]),
            ])]
        );
    }

    #[test]
    fn lists_nest_and_ordered_lists_keep_their_start() {
        assert_eq!(
            parse("3. three\n4. four\n   - inner"),
            [Node::List {
                start: Some(3),
                items: vec![
                    Item {
                        checked: None,
                        children: vec![text("three")],
                    },
                    Item {
                        checked: None,
                        children: vec![
                            text("four"),
                            Node::List {
                                start: None,
                                items: vec![Item {
                                    checked: None,
                                    children: vec![text("inner")],
                                }],
                            },
                        ],
                    },
                ],
            }]
        );
    }

    #[test]
    fn a_task_list_carries_its_checkmarks() {
        assert_eq!(
            parse("- [x] done\n- [ ] not yet"),
            [Node::List {
                start: None,
                items: vec![
                    Item {
                        checked: Some(true),
                        children: vec![text("done")],
                    },
                    Item {
                        checked: Some(false),
                        children: vec![text("not yet")],
                    },
                ],
            }]
        );
    }

    #[test]
    fn a_blockquote_holds_its_paragraphs() {
        assert_eq!(
            parse("> quoted words"),
            [Node::BlockQuote(vec![paragraph(vec![text(
                "quoted words"
            )])])]
        );
    }

    #[test]
    fn a_table_keeps_header_and_rows() {
        assert_eq!(
            parse("| name | speed |\n| --- | --- |\n| quick | `O(n log n)` |"),
            [Node::Table {
                header: vec![vec![text("name")], vec![text("speed")]],
                rows: vec![vec![vec![text("quick")], vec![code("O(n log n)")]]],
            }]
        );
    }

    #[test]
    fn a_thematic_break_is_a_rule() {
        assert_eq!(
            parse("above\n\n---\n\nbelow"),
            [
                paragraph(vec![text("above")]),
                Node::Rule,
                paragraph(vec![text("below")]),
            ]
        );
    }

    // ----- deliberately plain -----

    #[test]
    fn a_mermaid_fence_is_an_ordinary_code_block_with_its_info_kept() {
        assert_eq!(
            parse("```mermaid\ngraph TD;\n```"),
            [Node::CodeBlock {
                info: "mermaid".to_owned(),
                code: "graph TD;\n".to_owned(),
            }]
        );
    }

    #[test]
    fn dollar_math_is_ordinary_text() {
        assert_eq!(parse("$x^2$"), [paragraph(vec![text("$x^2$")])]);
    }

    #[test]
    fn an_unclosed_fence_still_renders_reasonably() {
        assert_eq!(
            parse("```rust\nlet x = 1;"),
            [Node::CodeBlock {
                info: "rust".to_owned(),
                code: "let x = 1;".to_owned(),
            }]
        );
    }

    #[test]
    fn a_code_block_never_autolinks() {
        assert_eq!(
            parse("```\nhttps://example.com\n```"),
            [Node::CodeBlock {
                info: String::new(),
                code: "https://example.com\n".to_owned(),
            }]
        );
    }

    // ----- the property: total, and markup-proof by type -----

    /// A deliberately hostile alphabet, walked pseudo-randomly: parse
    /// must be total, and — since the type system admits no raw-HTML
    /// node — every `<` that survives lives in a string field, which the
    /// view renders as a text node.
    #[test]
    fn arbitrary_input_never_panics_and_never_becomes_markup() {
        const ALPHABET: &[char] = &[
            '<', '>', '`', '*', '_', '[', ']', '(', ')', '#', '|', '-', '!', '$', '~', '\\', '"',
            '\'', '&', ':', '/', '\n', ' ', 'a', 'h', 't', 'p', 's', '.', '3',
        ];
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        for _ in 0..500 {
            let length = (state % 120) as usize;
            let input: String = (0..length)
                .map(|_| {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1);
                    ALPHABET[(state >> 33) as usize % ALPHABET.len()]
                })
                .collect();
            let _ = parse(&input);
        }
        // And the flagship injection, explicitly: the tag survives only
        // as text content.
        let nodes = parse("hello <script>alert(1)</script> there");
        fn only_data(nodes: &[Node]) -> bool {
            nodes.iter().all(|node| match node {
                Node::Text(_)
                | Node::Code(_)
                | Node::CodeBlock { .. }
                | Node::Elided(_)
                | Node::Rule => true,
                Node::Emphasis(c)
                | Node::Strong(c)
                | Node::Strikethrough(c)
                | Node::Paragraph(c)
                | Node::BlockQuote(c) => only_data(c),
                Node::Link { children, .. } | Node::Heading { children, .. } => only_data(children),
                Node::List { items, .. } => items.iter().all(|item| only_data(&item.children)),
                Node::Table { header, rows } => {
                    header.iter().all(|cell| only_data(cell))
                        && rows.iter().flatten().all(|cell| only_data(cell))
                }
            })
        }
        assert!(only_data(&nodes));
    }
}
