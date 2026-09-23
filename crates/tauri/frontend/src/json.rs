//! The one JSON renderer in the window: a folding, colored tree for
//! tool-call arguments, tool results, and fenced ```json blocks alike.
//!
//! The layout is [`epik_core::assistant::json`]'s: a [`Document`] is the
//! text parsed once, [`Document::rows`] lays it out as lines under a
//! fold set, and [`Document::initial_folds`] is the fold policy.
//! [`JsonTree`] is the one component, owning the
//! fold set as a signal and rendering lines only, so a card body and a
//! bubble each keep their own chrome. Each span's [`Kind`] wears
//! `leptos_rich_chat`'s highlight classes, so a tree is colored like the
//! code blocks around it.

use std::collections::HashSet;

use epik_core::assistant::json::{Document, Fold, Kind, Row};
use leptos::prelude::*;

/// The highlight classes a span of `kind` wears: the ones
/// `leptos_rich_chat` gives the same token in a highlighted JSON block.
const fn class(kind: Kind) -> &'static str {
    match kind {
        Kind::Plain => "",
        Kind::Key => "hl-entity hl-name hl-tag",
        Kind::String => "hl-string",
        Kind::Number => "hl-constant hl-numeric",
        Kind::Literal => "hl-constant hl-language",
        Kind::Note => "hl-comment",
    }
}

/// The tree, rendered as lines only — the caller's chrome around it.
/// Owns the fold set; every disclosure toggles one pointer.
///
/// Clicks are handled once, delegated on the line list, and the pointer
/// rides on the button as `data-pointer`: rows are rebuilt in place by
/// position when the set changes, and attributes are patched on rebuild
/// where a per-row closure would go stale.
#[component]
pub fn JsonTree(document: Document) -> impl IntoView {
    let folded = RwSignal::new(document.initial_folds());
    let document = StoredValue::new(document);
    let toggle = move |event: leptos::ev::MouseEvent| {
        let Some(pointer) = clicked_pointer(&event) else {
            return;
        };
        folded.update(|folds| toggle(folds, pointer));
    };
    view! {
        <div class="overflow-x-auto" on:click=toggle>
            {move || {
                let folds = folded.get();
                document
                    .with_value(|document| document.rows(&folds))
                    .into_iter()
                    .map(row_view)
                    .collect_view()
            }}
        </div>
    }
}

/// Folds what is open and opens what is folded — one node, by pointer.
/// The set holds what is folded, so membership is the whole state.
fn toggle(folds: &mut HashSet<String>, pointer: String) {
    if !folds.remove(&pointer) {
        folds.insert(pointer);
    }
}

/// The pointer of the disclosure control a click landed on, if it did.
fn clicked_pointer(event: &leptos::ev::MouseEvent) -> Option<String> {
    use wasm_bindgen::JsCast;
    let target = event.target()?;
    let element = target.dyn_ref::<web_sys::Element>()?;
    let button = element.closest("button[data-pointer]").ok()??;
    button.get_attribute("data-pointer")
}

/// The disclosure control's and its spacer's shared width class.
const GUTTER: &str = "inline-block w-4 shrink-0 text-center";

/// What a disclosure control reads as, to an eye and to a screen
/// reader: the triangle points the way the fold would go, and
/// `aria-expanded` says where it stands now.
const fn disclosure(folded: bool) -> (&'static str, &'static str) {
    if folded {
        ("▸", "false")
    } else {
        ("▾", "true")
    }
}

fn row_view(row: Row) -> AnyView {
    let control = match row.fold {
        Some(Fold { pointer, folded }) => {
            let (glyph, expanded) = disclosure(folded);
            view! {
                <button
                    type="button"
                    class=format!("{GUTTER} cursor-pointer opacity-70 hover:opacity-100")
                    aria-expanded=expanded
                    data-pointer=pointer
                >
                    {glyph}
                </button>
            }
            .into_any()
        }
        None => view! { <span class=GUTTER></span> }.into_any(),
    };
    let spans = row
        .spans
        .into_iter()
        .map(|span| view! { <span class=class(span.kind)>{span.text}</span> })
        .collect_view();
    view! {
        <div class="whitespace-pre" style:padding-left=format!("{}ch", row.depth * 2)>
            {control}
            {spans}
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_wears_the_classes_a_highlighted_json_block_gives_it() {
        assert_eq!(class(Kind::Key), "hl-entity hl-name hl-tag");
        assert_eq!(class(Kind::String), "hl-string");
        assert_eq!(class(Kind::Number), "hl-constant hl-numeric");
        assert_eq!(class(Kind::Literal), "hl-constant hl-language");
        assert_eq!(class(Kind::Note), "hl-comment");
    }

    #[test]
    fn punctuation_wears_no_class() {
        assert_eq!(class(Kind::Plain), "");
    }

    #[test]
    fn a_control_reads_the_same_way_to_an_eye_and_to_a_screen_reader() {
        let (folded_glyph, folded_aria) = disclosure(true);
        let (open_glyph, open_aria) = disclosure(false);
        assert_eq!(folded_aria, "false");
        assert_eq!(open_aria, "true");
        assert_ne!(
            folded_glyph, open_glyph,
            "the two states are told apart by sight too"
        );
    }

    #[test]
    fn a_click_folds_what_is_open_and_opens_what_is_folded() {
        let mut folds = HashSet::new();
        toggle(&mut folds, "/a".to_owned());
        assert!(folds.contains("/a"));
        toggle(&mut folds, "/a".to_owned());
        assert!(folds.is_empty());
    }

    #[test]
    fn one_click_moves_one_node() {
        let mut folds = HashSet::from(["/a".to_owned(), "/b".to_owned()]);
        toggle(&mut folds, "/a".to_owned());
        assert_eq!(folds, HashSet::from(["/b".to_owned()]));
        toggle(&mut folds, "/c".to_owned());
        assert_eq!(folds, HashSet::from(["/b".to_owned(), "/c".to_owned()]));
    }

    /// What the tree renders is whatever the document lays out under
    /// the set a click leaves behind — the component holds the set and
    /// nothing else, so a round trip through `toggle` is the whole of
    /// its state.
    #[test]
    fn a_click_and_a_click_back_leave_the_same_rows() {
        let document = Document::repaired(r#"{"outer": {"inner": 1}}"#).expect("a document");
        let mut folds = document.initial_folds();
        let shown = document.rows(&folds);
        toggle(&mut folds, "/outer".to_owned());
        assert_ne!(document.rows(&folds), shown, "the click moved something");
        toggle(&mut folds, "/outer".to_owned());
        assert_eq!(document.rows(&folds), shown);
    }
}
