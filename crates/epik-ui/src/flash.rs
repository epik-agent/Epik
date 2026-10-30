//! Flashing what changed: a running issue's line or card flashes when its
//! agent logs.

use std::collections::HashMap;

use leptos::html::ElementType;
use leptos::prelude::*;
use leptos::wasm_bindgen::JsCast;
use leptos::web_sys::HtmlElement;

/// Remembers each thing's last event, and says whether it changed since the
/// last time it was seen, so it can flash.
#[derive(Debug, Default)]
pub struct Flashes(HashMap<String, Option<i64>>);

impl Flashes {
    pub fn changed(&mut self, key: &str, last_event: Option<i64>) -> bool {
        let before = self.0.insert(key.to_owned(), last_event);
        matches!(before, Some(before) if last_event.is_some() && before != last_event)
    }
}

/// How many times each thing has flashed, by key. A window that flashes
/// provides it as context; without it nothing flashes.
#[derive(Debug, Clone, Copy)]
pub struct Flashing(pub RwSignal<HashMap<String, u64>>);

impl Flashing {
    /// Flashes each of `keys` once more.
    pub fn flash(self, keys: Vec<String>) {
        if keys.is_empty() {
            return;
        }
        self.0.update(|counts| {
            for key in keys {
                *counts.entry(key).or_default() += 1;
            }
        });
    }
}

/// Flashes the element at `node` each time `key` flashes. The element
/// stays, so its animation is started over.
pub fn flash_on<E>(node: NodeRef<E>, key: String)
where
    E: ElementType,
    E::Output: JsCast + Clone + 'static,
{
    let Some(Flashing(counts)) = use_context::<Flashing>() else {
        return;
    };
    let count = Memo::new(move |_| counts.with(|counts| counts.get(&key).copied().unwrap_or(0)));
    Effect::new(move |seen: Option<u64>| {
        let now = count.get();
        if seen.is_some_and(|seen| seen != now)
            && let Some(node) = node.get()
        {
            let node: &HtmlElement = node.unchecked_ref();
            let classes = node.class_list();
            let _ = classes.remove_1("flash");
            // Reading its width makes the browser notice the class went.
            let _ = node.offset_width();
            let _ = classes.add_1("flash");
        }
        now
    });
}
