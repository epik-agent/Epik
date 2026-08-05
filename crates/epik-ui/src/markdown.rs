//! Assistant turns are markdown.
//!
//! Rendering a model's output as HTML is a decision about what it is *not*
//! allowed to say, so this is a converter and a filter at once. Raw HTML is
//! dropped, and a link the webview should not follow is neutralized: a model
//! that has been talked into emitting a `<script>` tag or a `javascript:` href
//! must not thereby be running code in the window.
//!
//! Both decisions are pure, which is why they are here and tested here rather
//! than discovered in a browser.

use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, TagEnd, html};

/// What a link or an image may point at. Anything else is not a document, and
/// a relative path is not one either — in a webview there is nowhere for it to
/// go but away from the app.
const FOLLOWABLE: [&str; 4] = ["http://", "https://", "mailto:", "#"];

/// Where a destination that is not [`FOLLOWABLE`] is sent instead: nowhere,
/// visibly.
const NOWHERE: &str = "#";

/// One assistant turn, as HTML.
///
/// Text is escaped by the parser, so the only unescaped markup in the result is
/// markup this function chose to emit.
#[must_use]
pub fn to_html(markdown: &str) -> String {
    // Tables and strikethrough, because models emit them and they are plain
    // markup. Footnotes and the rest can be turned on when something wants
    // them.
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let events = Parser::new_ext(markdown, options)
        .filter(|event| !is_raw_html(event))
        .map(vetted);

    let mut rendered = String::new();
    html::push_html(&mut rendered, events);
    rendered
}

/// Whether this event is HTML the model wrote rather than markup the parser
/// derived. Dropped rather than escaped: showing a model's stray tag as text is
/// noise, and running it is worse.
const fn is_raw_html(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::Html(_)
            | Event::InlineHtml(_)
            | Event::Start(Tag::HtmlBlock)
            | Event::End(TagEnd::HtmlBlock)
    )
}

/// The same event with any destination the webview should not follow replaced.
fn vetted(event: Event<'_>) -> Event<'_> {
    match event {
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Link {
            link_type,
            dest_url: followable(dest_url),
            title,
            id,
        }),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Image {
            link_type,
            dest_url: followable(dest_url),
            title,
            id,
        }),
        other => other,
    }
}

fn followable(destination: CowStr<'_>) -> CowStr<'_> {
    let plain = destination.trim().to_ascii_lowercase();
    if FOLLOWABLE.iter().any(|scheme| plain.starts_with(scheme)) {
        destination
    } else {
        CowStr::Borrowed(NOWHERE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_markdown_becomes_ordinary_html() {
        let rendered = to_html("A **bold** claim.\n\n- one\n- two\n");
        assert!(rendered.contains("<strong>bold</strong>"), "{rendered}");
        assert!(rendered.contains("<li>one</li>"), "{rendered}");
    }

    #[test]
    fn a_fenced_block_becomes_code() {
        let rendered = to_html("```rust\nfn main() {}\n```\n");
        assert!(rendered.contains("<pre><code"), "{rendered}");
        assert!(rendered.contains("fn main()"), "{rendered}");
    }

    #[test]
    fn a_script_tag_the_model_emitted_does_not_survive() {
        let rendered = to_html("before\n\n<script>alert(1)</script>\n\nafter");
        assert!(!rendered.contains("<script"), "{rendered}");
        assert!(
            rendered.contains("before") && rendered.contains("after"),
            "the prose around it is still shown: {rendered}"
        );
    }

    #[test]
    fn inline_html_does_not_survive_either() {
        let rendered = to_html("a <img src=x onerror=alert(1)> b");
        assert!(!rendered.contains("onerror"), "{rendered}");
    }

    #[test]
    fn a_javascript_link_goes_nowhere() {
        let rendered = to_html("[click me](javascript:alert(1))");
        assert!(!rendered.contains("javascript"), "{rendered}");
        assert!(rendered.contains("href=\"#\""), "{rendered}");
        assert!(
            rendered.contains("click me"),
            "the text stays; only the destination is refused: {rendered}"
        );
    }

    #[test]
    fn a_javascript_link_dressed_up_still_goes_nowhere() {
        for dressed in [
            "[x](JavaScript:alert(1))",
            "[x]( javascript:alert(1))",
            "[x](vbscript:msgbox)",
            "[x](data:text/html;base64,PHNjcmlwdD4=)",
        ] {
            let rendered = to_html(dressed);
            assert!(rendered.contains("href=\"#\""), "{dressed} -> {rendered}");
        }
    }

    #[test]
    fn an_image_from_somewhere_it_should_not_be_is_refused_too() {
        let rendered = to_html("![x](javascript:alert(1))");
        assert!(!rendered.contains("javascript"), "{rendered}");
    }

    #[test]
    fn a_plain_web_link_is_left_alone() {
        let rendered = to_html("[the docs](https://epik.dev/docs)");
        assert!(
            rendered.contains("href=\"https://epik.dev/docs\""),
            "{rendered}"
        );
    }

    #[test]
    fn a_relative_link_goes_nowhere_because_there_is_nowhere_to_go() {
        let rendered = to_html("[a file](../etc/passwd)");
        assert!(rendered.contains("href=\"#\""), "{rendered}");
    }

    #[test]
    fn prose_that_looks_like_markup_is_escaped_rather_than_emitted() {
        let rendered = to_html("compare `a < b` with a < b");
        assert!(!rendered.contains("with a < b"), "{rendered}");
        assert!(rendered.contains("&lt;"), "{rendered}");
    }

    #[test]
    fn an_empty_turn_renders_to_nothing() {
        assert_eq!(to_html(""), "");
    }
}
