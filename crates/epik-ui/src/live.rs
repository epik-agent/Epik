//! Keeping a view's nodes while what it shows changes. The main window
//! redraws every second; a node that a refresh replaced would lose a click
//! whose press and release straddled it. So each part of a view follows
//! only the part of the view model it draws, and lists are keyed.

use leptos::prelude::*;

/// The part of `signal` that `part` picks. It changes only when that part
/// does, so what draws it is redrawn only then.
pub fn part<T, U>(signal: Signal<T>, part: impl Fn(&T) -> U + Send + Sync + 'static) -> Memo<U>
where
    T: Send + Sync + 'static,
    U: PartialEq + Send + Sync + 'static,
{
    Memo::new(move |_| signal.with(&part))
}

/// `initial`, kept up to date with what `find` finds: a keyed list's item,
/// followed as the list changes. While it finds nothing it keeps what it
/// had.
pub fn kept<T>(initial: T, find: impl Fn() -> Option<T> + Send + Sync + 'static) -> Memo<T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    Memo::new(move |before: Option<&T>| {
        find()
            .or_else(|| before.cloned())
            .unwrap_or_else(|| initial.clone())
    })
}

/// Each item of `list`, in a keyed list whose items follow it: an item's
/// nodes stay while its key does.
pub fn keyed<T, K, V>(
    list: Signal<Vec<T>>,
    key: fn(&T) -> K,
    draw: impl Fn(Signal<T>) -> V + Send + Sync + Clone + 'static,
) -> impl IntoView
where
    T: Clone + PartialEq + Send + Sync + 'static,
    K: Eq + std::hash::Hash + Clone + Send + Sync + 'static,
    V: IntoView + 'static,
{
    view! {
        <For each=move || list.get() key=key let(item)>
            {
                let at = key(&item);
                let item = kept(item, move || list.with(|list| list.iter().find(|item| key(item) == at).cloned()));
                draw(item.into())
            }
        </For>
    }
}
