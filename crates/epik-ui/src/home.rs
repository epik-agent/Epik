//! The main window's Home, the Runs page: a table of the runs, which tabs
//! filter by where they stand, whose building rows unfold to show what is
//! running, what starts next, and what is done.

use epik_view::{Disposition, Home, NowLine, RunCard, Words as Sentence};
use leptos::ev::MouseEvent;
use leptos::html::Div;
use leptos::prelude::*;

use crate::flash::flash_on;
use crate::graph::{MiniGraph, ProgressBar};
use crate::live::{keyed, part};
use crate::main_window::{disposition_class, href};
use crate::words::{GithubLink, Words};

/// What a row needing attention asks the app to do to its run: `rerun` or
/// `delete_run`.
pub type Act = (&'static str, String);

/// Which runs the table shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Building,
    Waiting,
    Attention,
}

impl Filter {
    fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Building => "Building",
            Filter::Waiting => "Waiting for review",
            Filter::Attention => "Needs attention",
        }
    }

    /// The runs it shows: those needing attention, then building, then
    /// waiting for review.
    fn runs(self, home: &Home) -> Vec<RunCard> {
        let all = self == Filter::All;
        let mut runs = Vec::new();
        if all || self == Filter::Attention {
            runs.extend(home.attention.iter().cloned());
        }
        if all || self == Filter::Building {
            runs.extend(home.building.iter().cloned());
        }
        if all || self == Filter::Waiting {
            runs.extend(home.waiting.iter().cloned());
        }
        runs
    }

    fn count(self, home: &Home) -> usize {
        match self {
            Filter::All => home.building.len() + home.waiting.len() + home.attention.len(),
            Filter::Building => home.building.len(),
            Filter::Waiting => home.waiting.len(),
            Filter::Attention => home.attention.len(),
        }
    }
}

fn life_class(life: &Option<String>) -> &'static str {
    match life {
        Some(life) if life.starts_with("quiet") => "life quiet",
        _ => "life",
    }
}

/// A running issue's line: what it's doing and its agent's life line. It
/// flashes when its agent logs.
fn now_line(run: String, line: Signal<NowLine>) -> impl IntoView {
    let row = NodeRef::<Div>::new();
    flash_on(
        row,
        format!("{run}#{}", line.with_untracked(|line| line.number)),
    );
    let words = part(line, |line| line.words.clone());
    let life = part(line, |line| line.life.clone());
    view! {
        <div class="line" node_ref=row>
            <span class="label">"Now"</span>
            {move || view! { <Words spans=words.get()/> }}
            <span class=move || life.with(life_class)>{move || life.get()}</span>
        </div>
    }
}

/// A line of a sentence, while there is one.
fn line(label: &'static str, sentence: Memo<Option<Sentence>>) -> impl IntoView {
    move || {
        sentence.get().map(|spans| {
            view! {
                <div class="line">
                    <span class="label">{label}</span>
                    <Words spans/>
                </div>
            }
        })
    }
}

/// A run's row: its title and repository, graph and progress, a dot and
/// its sentence, its time and cost, and Rerun and Delete if it needs
/// attention or else Open ›. A building run's row unfolds to its Now, Next
/// and Done lines.
fn run_row(
    card: Signal<RunCard>,
    unfolded: RwSignal<Vec<String>>,
    on_act: Option<Callback<Act>>,
) -> AnyView {
    let run = card.with_untracked(|card| card.run.clone());
    let disposition = part(card, |card| card.disposition);
    let building = Memo::new(move |_| disposition.get() == Disposition::Building);
    let open = {
        let run = run.clone();
        Memo::new(move |_| building.get() && unfolded.with(|unfolded| unfolded.contains(&run)))
    };
    let fold = {
        let run = run.clone();
        move |_: MouseEvent| {
            unfolded.update(|unfolded| match unfolded.iter().position(|at| *at == run) {
                Some(at) => {
                    unfolded.remove(at);
                }
                None => unfolded.push(run.clone()),
            })
        }
    };
    let page = part(card, |card| href(&format!("run/{}", card.run)));
    let title = part(card, |card| card.title.clone());
    let epic = part(card, |card| card.epic.clone());
    let graph = part(card, |card| card.graph.clone());
    let progress = part(card, |card| card.progress);
    let sentence = part(card, |card| {
        match card.disposition {
            Disposition::Building => card.summary.clone(),
            _ => card.sentence.clone(),
        }
        .unwrap_or_default()
    });
    let elapsed = part(card, |card| card.elapsed.clone());
    let cost = part(card, |card| card.cost.clone());
    let now = part(card, |card| card.now.clone());
    let actions = {
        let run = run.clone();
        move || {
            if disposition.get() != Disposition::NeedsAttention {
                return view! { <a href=page class="open">"Open ›"</a> }.into_any();
            }
            let act = |command: &'static str| {
                let run = run.clone();
                move |_: MouseEvent| {
                    if let Some(on_act) = on_act {
                        on_act.run((command, run.clone()));
                    }
                }
            };
            view! {
                <button class="primary" on:click=act("rerun")>"Rerun"</button>
                <button on:click=act("delete_run")>"Delete"</button>
            }
            .into_any()
        }
    };
    let lines = move || {
        let run = run.clone();
        open.get().then(|| {
            view! {
                <tr class="lines">
                    <td></td>
                    <td colspan="6">
                        {keyed(now.into(), |line: &NowLine| line.number, move |line| now_line(run.clone(), line))}
                        {line("Next", part(card, |card| card.next.clone()))}
                        {line("Done", part(card, |card| card.done.clone()))}
                    </td>
                </tr>
            }
        })
    };
    view! {
        <tbody class=move || format!("run {}", disposition_class(disposition.get()))>
            <tr>
                <td class="fold-cell">
                    {move || {
                        let fold = fold.clone();
                        building.get().then(|| view! {
                            <button
                                aria-label=move || if open.get() { "Fold" } else { "Unfold" }
                                aria-expanded=move || if open.get() { "true" } else { "false" }
                                class="fold"
                                on:click=fold
                            >
                                {move || if open.get() { "▾" } else { "▸" }}
                            </button>
                        })
                    }}
                </td>
                <td class="run-cell">
                    <a href=page class="title">{title}</a>
                    <div class="repo">
                        {move || {
                            let epic = epic.get();
                            view! { <GithubLink text=epic.text url=epic.url.unwrap_or_default()/> }
                        }}
                    </div>
                </td>
                <td class="progress-cell">
                    {move || view! { <MiniGraph graph=graph.get()/> }}
                    {move || view! { <ProgressBar progress=progress.get()/> }}
                </td>
                <td class="status">
                    <span class=move || format!("dot {}", disposition_class(disposition.get()))></span>
                    {move || view! { <Words spans=sentence.get()/> }}
                </td>
                <td class="figure">{elapsed}</td>
                <td class="figure">{cost}</td>
                <td class="actions">{actions}</td>
            </tr>
            {lines}
        </tbody>
    }
    .into_any()
}

fn run_key(card: &RunCard) -> String {
    card.run.clone()
}

/// Home, the Runs page: its title and the headline, tabs that filter the
/// runs, each with how many runs it shows, and the table of runs; or that
/// there are none. `filter` is the tab chosen at first, and `unfolded` the
/// building runs whose rows are unfolded at first.
#[component]
pub fn HomePage(
    #[prop(into)] home: Signal<Home>,
    #[prop(optional)] filter: Filter,
    #[prop(optional)] unfolded: Vec<String>,
    #[prop(optional)] on_act: Option<Callback<Act>>,
) -> impl IntoView {
    let filter = RwSignal::new(filter);
    let unfolded = RwSignal::new(unfolded);
    let headline = part(home, |home| home.headline.clone());
    let empty = part(home, |home| Filter::All.count(home) == 0);
    let runs = Memo::new(move |_| home.with(|home| filter.get().runs(home)));
    let tab = move |tab: Filter| {
        let count = part(home, move |home| tab.count(home));
        let chosen = Memo::new(move |_| filter.get() == tab);
        view! {
            <button
                role="tab"
                aria-selected=move || if chosen.get() { "true" } else { "false" }
                class="tab"
                on:click=move |_| filter.set(tab)
            >
                {tab.label()}
                <span class="count">{count}</span>
            </button>
        }
    };
    view! {
        <div class="page-head">
            <h1>"Runs"</h1>
            <p class="headline">{headline}</p>
        </div>
        {move || {
            if empty.get() {
                return view! {
                    <div class="empty">
                        <p>"No runs right now."</p>
                        <p class="muted">
                            "Ask Epik to build an issue or an epic in Claude Code, and it shows up here while it builds."
                        </p>
                    </div>
                }
                .into_any();
            }
            view! {
                <div role="tablist" class="tabs">
                    {tab(Filter::All)}
                    {tab(Filter::Building)}
                    {tab(Filter::Waiting)}
                    {tab(Filter::Attention)}
                </div>
                <table class="run-table">
                    <thead>
                        <tr>
                            <th></th>
                            <th>"Run"</th>
                            <th>"Progress"</th>
                            <th>"Status"</th>
                            <th class="figure">"Time"</th>
                            <th class="figure">"Cost"</th>
                            <th></th>
                        </tr>
                    </thead>
                    {keyed(runs.into(), run_key, move |card| run_row(card, unfolded, on_act))}
                </table>
            }
            .into_any()
        }}
    }
}
