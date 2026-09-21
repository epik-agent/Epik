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
        folded.update(|folds| {
            if !folds.remove(&pointer) {
                folds.insert(pointer);
            }
        });
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

fn row_view(row: Row) -> AnyView {
    let control = match row.fold {
        Some(Fold { pointer, folded }) => view! {
            <button
                type="button"
                class=format!("{GUTTER} cursor-pointer opacity-70 hover:opacity-100")
                aria-expanded=if folded { "false" } else { "true" }
                data-pointer=pointer
            >
                {if folded { "▸" } else { "▾" }}
            </button>
        }
        .into_any(),
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
}
