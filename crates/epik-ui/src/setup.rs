//! The **Set up Epik** window: each thing Epik needs, and a button for each
//! that isn't done. Nothing runs until its button is pressed. While a step
//! runs its output is polled and shown, and when it ends the checklist is
//! checked again.

use std::collections::BTreeMap;
use std::time::Duration;

use epik_view::{Checklist, ItemState, Output, Row};
use leptos::html::Pre;
use leptos::prelude::*;
use serde::Serialize;

use crate::tauri;

const POLL: Duration = Duration::from_millis(400);

/// What the window shows: the checklist, once fetched, each item's latest
/// step's output, and the error line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Setup {
    pub checklist: Option<Checklist>,
    /// Each item's latest step, by id: the one running, or what an ended
    /// one printed, kept across redraws.
    pub outputs: BTreeMap<String, Output>,
    pub error: String,
}

impl Setup {
    /// The checklist was fetched, or couldn't be. A failure keeps the
    /// checklist it had.
    pub fn fetched(&mut self, checklist: Result<Checklist, String>) {
        match checklist {
            Ok(checklist) => {
                self.checklist = Some(checklist);
                self.error.clear();
            }
            Err(reason) => self.error = format!("Couldn't check setup: {reason}"),
        }
    }

    /// The app wouldn't start a step, for `reason`.
    pub fn refused(&mut self, reason: String) {
        self.error = reason;
    }

    /// The step of item `id` started: its previous output is gone.
    pub fn started(&mut self, id: &str) {
        let running = Output {
            running: true,
            ..Output::default()
        };
        self.outputs.insert(id.to_owned(), running);
    }

    /// The step of item `id` has printed `output`. Whether it still runs.
    pub fn followed(&mut self, id: &str, output: Output) -> bool {
        let running = output.running;
        self.outputs.insert(id.to_owned(), output);
        running
    }

    /// Whether a step is running.
    pub fn running(&self) -> bool {
        self.outputs.values().any(|output| output.running)
    }
}

fn mark(state: ItemState) -> &'static str {
    match state {
        ItemState::Done => "✓",
        ItemState::Missing => "✕",
        ItemState::Outdated => "!",
    }
}

fn state_class(state: ItemState) -> &'static str {
    match state {
        ItemState::Done => "item done",
        ItemState::Missing => "item missing",
        ItemState::Outdated => "item outdated",
    }
}

/// An item: its state's mark, title, button and detail, then its latest
/// step's output and failure. Its button is disabled while `busy`.
#[component]
pub fn Item(
    row: Row,
    busy: bool,
    #[prop(optional_no_strip)] output: Option<Output>,
    #[prop(optional_no_strip)] on_press: Option<Callback<String>>,
) -> impl IntoView {
    let id = row.id.clone();
    let button = row.button.map(|label| {
        let press = move |_| {
            if let Some(on_press) = on_press {
                on_press.run(id.clone());
            }
        };
        view! { <button class="primary" disabled=busy on:click=press>{label}</button> }
    });
    let output = output.map(|output| {
        let log = NodeRef::<Pre>::new();
        // Show the end of what it printed.
        Effect::new(move || {
            if let Some(log) = log.get() {
                log.set_scroll_top(log.scroll_height());
            }
        });
        let failed = output
            .error
            .map(|error| view! { <span class="failed">{error}</span> });
        view! {
            <pre class="log" node_ref=log>{output.lines.join("\n")}</pre>
            {failed}
        }
    });
    view! {
        <li class=state_class(row.state) data-id=row.id>
            <span class="mark">{mark(row.state)}</span>
            <span class="title">{row.title}</span>
            {button}
            <span class="detail">{row.detail}</span>
            {output}
        </li>
    }
}

/// The window's main part: the items, the complete message once every item
/// is done, and the error line.
#[component]
pub fn Items(setup: Setup, #[prop(optional)] on_press: Option<Callback<String>>) -> impl IntoView {
    let busy = setup.running();
    let complete = setup.checklist.as_ref().is_some_and(|list| list.complete);
    let mut outputs = setup.outputs;
    let items = setup
        .checklist
        .map(|list| list.rows)
        .unwrap_or_default()
        .into_iter()
        .map(|row| {
            let output = outputs.remove(&row.id);
            view! { <Item row busy output on_press/> }
        })
        .collect_view();
    view! {
        <ol id="items">{items}</ol>
        <section id="complete" hidden=!complete>
            <h2>"Epik is set up."</h2>
            <p>"Start Claude Code and say “Hello, Epik.”"</p>
        </section>
        <p id="error">{setup.error}</p>
    }
}

#[derive(Serialize)]
struct Step {
    id: String,
}

/// The setup window, which checks the checklist when it opens.
#[component]
pub fn SetupWindow() -> impl IntoView {
    let setup = RwSignal::new(Setup::default());
    let press = Callback::new(move |id: String| {
        leptos::task::spawn_local(async move {
            let step = Step { id };
            if let Err(reason) = tauri::invoke::<_, ()>("setup_run", &step).await {
                setup.update(|setup| setup.refused(reason));
                return;
            }
            setup.update(|setup| setup.started(&step.id));
            follow(setup, step);
        });
    });
    leptos::task::spawn_local(refresh(setup));
    view! {
        <header>
            <h1>"Set up Epik"</h1>
            <p>
                "Epik needs these on your Mac or PC. Each button says what it will do, and nothing happens until you press it. Nothing is installed system-wide."
            </p>
        </header>
        <main>{move || view! { <Items setup=setup.get() on_press=press/> }}</main>
    }
}

/// Fetches the checklist again.
async fn refresh(setup: RwSignal<Setup>) {
    let checklist = tauri::invoke::<_, Checklist>("setup_checklist", &()).await;
    setup.update(|setup| setup.fetched(checklist));
}

/// Shows what `step` has printed, and polls again while it runs. When it
/// ends, checks the checklist again.
fn follow(setup: RwSignal<Setup>, step: Step) {
    leptos::task::spawn_local(async move {
        let running = match tauri::invoke::<_, Output>("setup_output", &step).await {
            Ok(output) => setup
                .try_update(|setup| setup.followed(&step.id, output))
                .unwrap_or(false),
            Err(reason) => {
                setup.update(|setup| {
                    if let Some(output) = setup.outputs.get_mut(&step.id) {
                        output.running = false;
                    }
                    setup.refused(reason);
                });
                false
            }
        };
        if running {
            set_timeout(move || follow(setup, step), POLL);
        } else {
            refresh(setup).await;
        }
    });
}
