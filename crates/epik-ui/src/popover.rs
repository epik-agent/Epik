//! The tray's popover: each building run, the runs waiting for the user,
//! and a way into Epik's window. It updates every second while open.

use std::collections::HashSet;
use std::time::Duration;

use epik_view::{Disposition, NowLine, Popover, RunCard};
use leptos::ev::MouseEvent;
use leptos::prelude::*;
use serde::Serialize;

use crate::flash::Flashes;
use crate::graph::{MiniGraph, ProgressBar};
use crate::tauri;
use crate::words::Words;

const REFRESH: Duration = Duration::from_secs(1);

#[derive(Serialize)]
struct OpenMain {
    route: String,
}

/// Opens the main window at `route`.
fn open_main(route: String) {
    tauri::run("open_main", OpenMain { route });
}

/// The running issue whose agent logged most recently: the first with the
/// greatest `last_event`, once one has logged.
pub fn latest(now: &[NowLine]) -> Option<&NowLine> {
    now.iter().fold(None, |latest, line| {
        let at = |line: Option<&NowLine>| line.and_then(|line| line.last_event).unwrap_or(0);
        if at(Some(line)) > at(latest) {
            Some(line)
        } else {
            latest
        }
    })
}

/// The building runs whose cards flash: those whose latest agent's last
/// event changed.
fn flashing(flashes: &mut Flashes, popover: &Popover) -> HashSet<String> {
    popover
        .building
        .iter()
        .filter(|run| match latest(&run.now) {
            Some(line) if line.life.is_some() => flashes.changed(&run.run, line.last_event),
            _ => false,
        })
        .map(|run| run.run.clone())
        .collect()
}

/// A building run: its graph, title, time and summary, its latest agent's
/// life line, and its progress.
#[component]
pub fn Card(run: RunCard, flash: bool) -> impl IntoView {
    let route = format!("run/{}", run.run);
    let open = move |event: MouseEvent| {
        event.prevent_default();
        open_main(route.clone());
    };
    let life = latest(&run.now)
        .and_then(|line| line.life.clone())
        .map(|life| {
            let class = if life.starts_with("quiet") {
                "life quiet"
            } else {
                "life"
            };
            view! { <div class=class>{life}</div> }
        });
    let progress =
        (run.graph.layout.places.len() > 1).then(|| view! { <ProgressBar progress=run.progress/> });
    view! {
        <article class=if flash { "card flash" } else { "card" }>
            <div class="head">
                <MiniGraph graph=run.graph/>
                <a class="title" href="#" on:click=open>{run.title}</a>
                <span class="time">{run.elapsed}</span>
            </div>
            <Words spans=run.summary.unwrap_or_default() class="summary"/>
            {life}
            {progress}
        </article>
    }
}

/// What the popover lists: a card for each building run, then the runs
/// waiting for the user; or that nothing is building.
#[component]
pub fn Runs(popover: Popover, #[prop(optional)] flashing: HashSet<String>) -> impl IntoView {
    if popover.building.is_empty() && popover.waiting.is_empty() {
        return view! { <p class="quiet-note">"Nothing is building."</p> }.into_any();
    }
    let cards = popover
        .building
        .into_iter()
        .map(|run| {
            let flash = flashing.contains(&run.run);
            view! { <Card run flash/> }
        })
        .collect_view();
    let waiting = (!popover.waiting.is_empty()).then(|| {
        let items = popover
            .waiting
            .into_iter()
            .map(|(disposition, _, sentence)| {
                view! {
                    <li class=disposition_class(disposition)>
                        <span class="marker"></span>
                        <Words spans=sentence/>
                    </li>
                }
            })
            .collect_view();
        view! {
            <h2>"Waiting for you"</h2>
            <ul class="waiting">{items}</ul>
        }
    });
    view! { {cards} {waiting} }.into_any()
}

fn disposition_class(disposition: Disposition) -> &'static str {
    match disposition {
        Disposition::Building => "building",
        Disposition::WaitingForReview => "waiting_for_review",
        Disposition::NeedsAttention => "needs_attention",
    }
}

/// Epik's mark, set as the span's HTML, as it is drawn everywhere else:
/// Leptos would wrap the static SVG's children in groups.
const MARK: &str = r##"<svg viewBox="0 0 64 64" width="18" height="18" aria-hidden="true"><g stroke="#171717" stroke-width="3" opacity="0.3"><line x1="16" y1="16" x2="48" y2="24"/><line x1="16" y1="16" x2="24" y2="48"/><line x1="48" y1="24" x2="40" y2="48"/><line x1="24" y1="48" x2="40" y2="48"/></g><circle cx="16" cy="16" r="7" fill="#00c07a"/><circle cx="48" cy="24" r="6" fill="#171717"/><circle cx="24" cy="48" r="6" fill="#171717"/><circle cx="40" cy="48" r="5" fill="#00c07a" opacity="0.7"/></svg>"##;

/// What a refresh drew: the popover and the cards that flash, or why it
/// couldn't be had. Each refresh is drawn afresh, as its own generation, so
/// a card that flashes again starts its flash over.
#[derive(Clone)]
struct Drawn {
    generation: u64,
    runs: Result<(Popover, HashSet<String>), String>,
}

/// The popover's window, refreshed from the app every second.
#[component]
pub fn PopoverWindow() -> impl IntoView {
    let headline = RwSignal::new(String::new());
    let drawn = RwSignal::new(None::<Drawn>);
    let flashes = StoredValue::new_local(Flashes::default());
    let refresh = move || {
        leptos::task::spawn_local(async move {
            let runs = match tauri::invoke::<_, Popover>("popover", &()).await {
                Ok(popover) => {
                    headline.set(popover.headline.clone());
                    let flashing = flashes.try_update_value(|flashes| flashing(flashes, &popover));
                    Ok((popover, flashing.unwrap_or_default()))
                }
                Err(reason) => Err(reason),
            };
            let generation = drawn
                .with_untracked(|drawn| drawn.as_ref().map_or(0, |drawn| drawn.generation + 1));
            drawn.set(Some(Drawn { generation, runs }));
        });
    };
    refresh();
    set_interval(refresh, REFRESH);
    let open = |event: MouseEvent| {
        event.prevent_default();
        open_main(String::new());
    };
    let quit = |event: MouseEvent| {
        event.prevent_default();
        tauri::run("quit", ());
    };
    view! {
        <header>
            <span id="mark" inner_html=MARK></span>
            <span class="name">"Epik"</span>
            <span id="headline">{headline}</span>
        </header>
        <main id="runs">
            <For each=move || drawn.get() key=|drawn| drawn.generation let(drawn)>
                {match drawn.runs {
                    Ok((popover, flashing)) => view! { <Runs popover flashing/> }.into_any(),
                    Err(reason) => view! { <p class="error">{reason}</p> }.into_any(),
                }}
            </For>
        </main>
        <footer>
            <a href="#" id="open" on:click=open>"Open Epik"</a>
            <a href="#" id="quit" on:click=quit>"Quit Epik"</a>
        </footer>
    }
}
