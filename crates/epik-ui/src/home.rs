//! The main window's Home: sections of run cards for the runs building,
//! waiting for review, and needing attention.

use epik_view::{Home, NowLine, RunCard, Words as Sentence};
use leptos::html::Div;
use leptos::prelude::*;

use crate::flash::flash_on;
use crate::graph::{MiniGraph, ProgressBar};
use crate::live::{keyed, part};
use crate::main_window::href;
use crate::words::{GithubLink, Words};

/// What a card needing attention asks the app to do to its run: `rerun`
/// or `delete_run`.
pub type Act = (&'static str, String);

/// A card's mini graph, redrawn when it changes.
fn graph(card: Signal<RunCard>) -> impl IntoView {
    let graph = part(card, |card| card.graph.clone());
    move || view! { <MiniGraph graph=graph.get()/> }
}

/// A card's title, linking to the run's page.
fn title(card: Signal<RunCard>) -> impl IntoView {
    let page = part(card, |card| href(&format!("run/{}", card.run)));
    let title = part(card, |card| card.title.clone());
    view! { <a class="title" href=page>{title}</a> }
}

/// A card's epic, linked to GitHub.
fn epic(card: Signal<RunCard>) -> impl IntoView {
    let epic = part(card, |card| card.epic.clone());
    move || {
        let epic = epic.get();
        view! { <GithubLink text=epic.text url=epic.url.unwrap_or_default()/> }
    }
}

/// A sentence, redrawn when it changes.
fn words(sentence: Memo<Sentence>) -> impl IntoView {
    move || view! { <Words spans=sentence.get()/> }
}

fn time(card: Signal<RunCard>) -> impl IntoView {
    let elapsed = part(card, |card| card.elapsed.clone());
    let cost = part(card, |card| card.cost.clone());
    view! {
        <span class="time">{elapsed}</span>
        <span class="cost">{cost}</span>
    }
}

fn life_class(life: &Option<String>) -> &'static str {
    match life {
        Some(life) if life.starts_with("quiet") => "life quiet",
        _ => "life",
    }
}

/// A running issue's row: what it's doing and its agent's life line. It
/// flashes when its agent logs.
fn now_row(run: String, line: Signal<NowLine>) -> impl IntoView {
    let row = NodeRef::<Div>::new();
    flash_on(
        row,
        format!("{run}#{}", line.with_untracked(|line| line.number)),
    );
    let life = part(line, |line| line.life.clone());
    view! {
        <div class="line" node_ref=row>
            <span class="label">"Now"</span>
            {words(part(line, |line| line.words.clone()))}
            <span class=move || life.with(life_class)>{move || life.get()}</span>
        </div>
    }
}

/// A row of a sentence, while there is one.
fn row(label: &'static str, sentence: Memo<Option<Sentence>>) -> impl IntoView {
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

/// A building run's card: its graph, title, figures and progress, and a
/// row for each running issue, what starts next, and what is done.
#[component]
pub fn BuildingCard(#[prop(into)] card: Signal<RunCard>) -> impl IntoView {
    let run = card.with_untracked(|card| card.run.clone());
    let page = part(card, |card| href(&format!("run/{}", card.run)));
    let count = part(card, |card| format!(" · {}", card.count));
    let progress = part(card, |card| card.progress);
    let now = part(card, |card| card.now.clone());
    view! {
        <article class="card building">
            <div class="card-head">
                {graph(card)}
                <div class="titles">
                    {title(card)}
                    <div class="sub">{epic(card)}{count}</div>
                </div>
                <div class="figures">
                    {time(card)}
                    <a class="open" href=page>"Open ›"</a>
                </div>
            </div>
            {move || view! { <ProgressBar progress=progress.get()/> }}
            <div class="rows">
                {keyed(now.into(), |line: &NowLine| line.number, move |line| now_row(run.clone(), line))}
                {row("Next", part(card, |card| card.next.clone()))}
                {row("Done", part(card, |card| card.done.clone()))}
            </div>
        </article>
    }
}

/// A card for a run that has ended: waiting for review, or needing
/// attention, when it has Rerun and Delete buttons.
#[component]
pub fn EndedCard(
    #[prop(into)] card: Signal<RunCard>,
    attention: bool,
    #[prop(optional_no_strip)] on_act: Option<Callback<Act>>,
) -> impl IntoView {
    let run = card.with_untracked(|card| card.run.clone());
    let note = part(card, |card| card.note.clone());
    let sentence = part(card, |card| card.sentence.clone().unwrap_or_default());
    let buttons = attention.then(|| {
        let act = move |command: &'static str| {
            let run = run.clone();
            move |_| {
                if let Some(on_act) = on_act {
                    on_act.run((command, run.clone()));
                }
            }
        };
        view! {
            <button class="primary" on:click=act("rerun")>"Rerun"</button>
            <button on:click=act("delete_run")>"Delete"</button>
        }
    });
    view! {
        <article class=if attention { "card attention" } else { "card waiting" }>
            <div class="card-head">
                {graph(card)}
                <div class="titles">
                    {title(card)}
                    <div class=if attention { "sub said bad" } else { "sub said good" }>
                        {epic(card)}" · "{words(sentence)}
                    </div>
                    {move || note.get().map(|note| view! { <div class="note">{note}</div> })}
                </div>
                <div class="figures">{time(card)}{buttons}</div>
            </div>
        </article>
    }
}

/// A section's heading, while it has cards.
fn heading(cards: Memo<Vec<RunCard>>, class: &'static str, title: &'static str) -> impl IntoView {
    move || (!cards.with(Vec::is_empty)).then(|| view! { <h2 class=class>{title}</h2> })
}

fn run_key(card: &RunCard) -> String {
    card.run.clone()
}

/// Home: the headline, top right, then a section for each kind of run
/// that has any, or that there are none.
#[component]
pub fn HomePage(
    #[prop(into)] home: Signal<Home>,
    #[prop(optional)] on_act: Option<Callback<Act>>,
) -> impl IntoView {
    let headline = part(home, |home| home.headline.clone());
    let building = part(home, |home| home.building.clone());
    let waiting = part(home, |home| home.waiting.clone());
    let attention = part(home, |home| home.attention.clone());
    let empty = part(home, |home| {
        home.building.is_empty() && home.waiting.is_empty() && home.attention.is_empty()
    });
    view! {
        <p class="headline">{headline}</p>
        {heading(building, "section", "Building")}
        {keyed(building.into(), run_key, |card| view! { <BuildingCard card/> })}
        {heading(waiting, "section", "Waiting for your review")}
        {keyed(waiting.into(), run_key, |card| view! { <EndedCard card attention=false/> })}
        {heading(attention, "section bad", "Needs attention")}
        {keyed(attention.into(), run_key, move |card| {
            view! { <EndedCard card attention=true on_act/> }
        })}
        {move || {
            empty
                .get()
                .then(|| {
                    view! {
                        <div class="empty">
                            <p>"No runs right now."</p>
                            <p class="muted">
                                "Ask Epik to build an issue or an epic in Claude Code, and it shows up here while it builds."
                            </p>
                        </div>
                    }
                })
        }}
    }
}
