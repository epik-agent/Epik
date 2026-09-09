//! Syntax highlighting for fenced code blocks, dispatched on the
//! fence's info string.
//!
//! [`highlight`] is a pure function: (language token, code) → classed
//! chunks, which the renderer maps into `<span class>` views — never
//! HTML, so the renderer's zero-sinks invariant stands, and classes
//! rather than inline styles, so the light and dark palettes are CSS.
//! An unknown, empty, or unresolvable language is `None`: the caller's
//! existing plain monospace block, unchanged. Never an error, never a
//! panic.
//!
//! The grammar set is the pruned dump `build.rs` compiles from
//! `grammars/` — exactly [`LANGUAGES`], nothing more — deserialized
//! once, lazily, on the first highlight. Completed messages are the
//! only callers (streaming deltas stay plain text by standing design),
//! so none of this sits on the streaming hot path. A pathological block
//! degrades instead of freezing: past [`CAP`], the tail renders
//! unhighlighted.

use std::sync::OnceLock;

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};

/// The supported languages, by the token a fence would name them with.
/// Aliases (rs, py, sh, ...) resolve through the grammars' own names
/// and file extensions; this list is the set the drift test pins
/// against the built dump — its only reader, which is the point.
#[cfg_attr(not(test), allow(dead_code))]
pub const LANGUAGES: [&str; 18] = [
    "rust",
    "python",
    "javascript",
    "typescript",
    "go",
    "java",
    "c",
    "c++",
    "bash",
    "sql",
    "json",
    "yaml",
    "toml",
    "html",
    "css",
    "markdown",
    "haskell",
    "lean",
];

/// How much of a block gets highlighted. Generous — past it, the rest
/// of the block renders plain rather than freezing the UI.
pub const CAP: usize = 64 * 1024;

/// One run of code: its highlight class, or `None` for plain text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chunk {
    pub class: Option<&'static str>,
    pub text: String,
}

/// Highlights `code` as `language`, or `None` when the language is
/// unknown — the caller renders its plain block exactly as before.
pub fn highlight(language: &str, code: &str) -> Option<Vec<Chunk>> {
    if language.is_empty() {
        return None;
    }
    // The grammars' own names and extensions resolve almost every
    // spelling (rs, py, sh, ...); "shell" is the one common token no
    // grammar claims.
    let language = if language.eq_ignore_ascii_case("shell") {
        "bash"
    } else {
        language
    };
    let set = syntaxes();
    let syntax = set.find_syntax_by_token(language)?;
    let (head, tail) = split_at_cap(code);
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut parser = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    for line in head.split_inclusive('\n') {
        // A line the grammar cannot parse falls back to the plain
        // block; nothing a message says may become an error.
        let ops = parser.parse_line(line, set).ok()?;
        let mut last = 0;
        for (index, op) in ops {
            emit(&mut chunks, classify(&stack), &line[last..index]);
            last = index;
            let _ = stack.apply(&op);
        }
        emit(&mut chunks, classify(&stack), &line[last..]);
    }
    emit(&mut chunks, None, tail);
    Some(chunks)
}

fn emit(chunks: &mut Vec<Chunk>, class: Option<&'static str>, text: &str) {
    if text.is_empty() {
        return;
    }
    // Adjacent runs of one class collapse into one chunk — fewer spans
    // in the DOM, and stabler goldens.
    if let Some(last) = chunks.last_mut()
        && last.class == class
    {
        last.text.push_str(text);
        return;
    }
    chunks.push(Chunk {
        class,
        text: text.to_owned(),
    });
}

/// Splits at the last line boundary inside [`CAP`]. A single line
/// longer than the cap highlights nothing — all tail, all plain.
fn split_at_cap(code: &str) -> (&str, &str) {
    if code.len() <= CAP {
        return (code, "");
    }
    match code[..CAP].rfind('\n') {
        Some(at) => code.split_at(at + 1),
        None => ("", code),
    }
}

/// The class for the innermost scope anything cares about. The palette
/// is deliberately small: seven classes cover what reads well in a
/// bubble; everything else stays plain.
fn classify(stack: &ScopeStack) -> Option<&'static str> {
    for scope in stack.scopes.iter().rev() {
        for (selector, class) in selectors() {
            if selector.is_prefix_of(*scope) {
                return Some(class);
            }
        }
    }
    None
}

fn selectors() -> &'static [(Scope, &'static str)] {
    static SELECTORS: OnceLock<Vec<(Scope, &'static str)>> = OnceLock::new();
    SELECTORS.get_or_init(|| {
        [
            ("comment", "hl-cm"),
            ("string", "hl-st"),
            ("constant.numeric", "hl-nm"),
            ("constant", "hl-cn"),
            ("keyword", "hl-kw"),
            ("storage", "hl-kw"),
            ("entity.name.function", "hl-fn"),
            ("support.function", "hl-fn"),
            ("variable.function", "hl-fn"),
            ("entity.name.tag", "hl-kw"),
            ("entity.other.attribute-name", "hl-fn"),
            ("entity.name", "hl-ty"),
            ("support.type", "hl-ty"),
            ("support.class", "hl-ty"),
        ]
        .into_iter()
        .map(|(selector, class)| {
            (
                Scope::new(selector).expect("the selector spellings are scopes"),
                class,
            )
        })
        .collect()
    })
}

/// The pruned SyntaxSet, deserialized once on first use — never per
/// message, never at startup.
fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(|| {
        syntect::dumps::from_binary(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/syntaxes.packdump"
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> Chunk {
        Chunk {
            class: None,
            text: text.to_owned(),
        }
    }

    fn classed(class: &'static str, text: &str) -> Chunk {
        Chunk {
            class: Some(class),
            text: text.to_owned(),
        }
    }

    #[test]
    fn a_rust_snippet_highlights_into_classed_chunks() {
        assert_eq!(
            highlight("rust", "fn main() { let x = 1; } // done\n").unwrap(),
            [
                classed("hl-kw", "fn"),
                plain(" "),
                classed("hl-fn", "main"),
                plain("() { "),
                classed("hl-kw", "let"),
                plain(" x "),
                classed("hl-kw", "="),
                plain(" "),
                classed("hl-nm", "1"),
                plain("; } "),
                classed("hl-cm", "// done\n"),
            ]
        );
    }

    #[test]
    fn a_haskell_snippet_highlights_into_classed_chunks() {
        assert_eq!(
            highlight("haskell", "quicksort [] = []  -- base\n").unwrap(),
            [
                plain("quicksort [] "),
                classed("hl-kw", "="),
                plain(" []  "),
                classed("hl-cm", "-- base\n"),
            ]
        );
    }

    #[test]
    fn a_lean_snippet_highlights_into_classed_chunks() {
        assert_eq!(
            highlight("lean", "def four : Nat := 4\n").unwrap(),
            [
                classed("hl-kw", "def"),
                plain(" "),
                classed("hl-fn", "four"),
                plain(" "),
                classed("hl-kw", ":"),
                plain(" "),
                classed("hl-kw", "Nat"),
                plain(" "),
                classed("hl-kw", ":="),
                plain(" "),
                classed("hl-nm", "4"),
                plain("\n"),
            ]
        );
    }

    #[test]
    fn unknown_and_empty_languages_fall_back_to_the_plain_block() {
        assert_eq!(highlight("brainfuck", "+++"), None);
        assert_eq!(highlight("", "fn main() {}"), None);
    }

    #[test]
    fn common_aliases_resolve() {
        for alias in ["rs", "py", "sh", "shell", "js", "ts", "cpp"] {
            assert!(highlight(alias, "x\n").is_some(), "{alias}");
        }
    }

    /// The chunks carry source text as data; anything markup-shaped in
    /// the code stays exactly the bytes it was.
    #[test]
    fn script_looking_code_stays_text_inside_chunks() {
        let chunks = highlight("html", "<script>alert(1)</script>\n").unwrap();
        let flattened: String = chunks.iter().map(|chunk| chunk.text.as_str()).collect();
        assert_eq!(flattened, "<script>alert(1)</script>\n");
    }

    #[test]
    fn past_the_cap_the_tail_is_one_plain_chunk() {
        let line = format!("let x = 1; // {}\n", "y".repeat(100));
        let big: String = std::iter::repeat_n(line.as_str(), CAP / line.len() + 10).collect();
        let chunks = highlight("rust", &big).unwrap();
        let tail = chunks.last().unwrap();
        assert_eq!(tail.class, None);
        assert!(
            tail.text.len() > line.len(),
            "the whole remainder rides one plain chunk"
        );
        let highlighted: usize = chunks[..chunks.len() - 1]
            .iter()
            .map(|chunk| chunk.text.len())
            .sum();
        assert!(highlighted <= CAP, "{highlighted}");
    }

    #[test]
    fn a_single_line_longer_than_the_cap_is_entirely_plain() {
        let one_line = format!("let x = \"{}\";", "z".repeat(CAP));
        assert_eq!(highlight("rust", &one_line).unwrap(), [plain(&one_line)]);
    }

    /// The drift test: every language the const list promises resolves
    /// in the built dump — and actually highlights a line, which forces
    /// its regexes to compile under fancy-regex.
    #[test]
    fn every_supported_language_resolves_and_compiles() {
        for language in LANGUAGES {
            let chunks = highlight(language, "x = 1\n");
            assert!(chunks.is_some(), "{language} is missing from the dump");
        }
    }
}
