//! A run's page: its issues laid out as their dependency graph, and the
//! chosen issue's detail panel.

use epik_view::{Disposition, IssueCard, NodeState, RunPage, Span, Stream, Words as Sentence};
use leptos::ev::MouseEvent;
use leptos::html::Button;
use leptos::prelude::*;

use crate::flash::flash_on;
use crate::live::{kept, keyed, part};
use crate::main_window::RunBar;
use crate::stream::StreamView;
use crate::words::{GithubLink, Words};

/// An issue card's size, and the gaps between cards.
const CARD_W: usize = 200;
const CARD_H: usize = 76;
const GAP_X: usize = 56;
const GAP_Y: usize = 18;

/// Where an issue card's left and top edges are.
fn place(column: usize, row: usize) -> (usize, usize) {
    (column * (CARD_W + GAP_X), row * (CARD_H + GAP_Y))
}

fn state_class(state: NodeState) -> &'static str {
    match state {
        NodeState::Merged => "merged",
        NodeState::Running => "running",
        NodeState::NotStarted => "not_started",
        NodeState::Failed => "failed",
    }
}

fn disposition_class(disposition: Disposition) -> &'static str {
    match disposition {
        Disposition::Building => "building",
        Disposition::WaitingForReview => "waiting_for_review",
        Disposition::NeedsAttention => "needs_attention",
    }
}

/// A sentence, redrawn when it changes.
fn words(sentence: Memo<Sentence>, class: Option<String>) -> impl IntoView {
    move || match class.clone() {
        Some(class) => view! { <Words spans=sentence.get() class/> }.into_any(),
        None => view! { <Words spans=sentence.get()/> }.into_any(),
    }
}

/// A link to a GitHub page, redrawn when it changes.
fn link(link: Memo<Span>) -> impl IntoView {
    move || {
        let link = link.get();
        view! { <GithubLink text=link.text url=link.url.unwrap_or_default()/> }
    }
}

/// An issue's number, linked to GitHub.
fn number(issue: Signal<IssueCard>) -> impl IntoView {
    link(part(issue, |issue| Span {
        text: format!("#{}", issue.number),
        url: Some(issue.url.clone()),
        code: false,
    }))
}

/// An issue's card, placed in the graph by its column and row. Choosing
/// it shows its detail panel. A running issue's card flashes when its
/// agent logs.
#[component]
pub fn Issue(
    #[prop(into)] issue: Signal<IssueCard>,
    #[prop(into)] chosen: Signal<bool>,
    /// The run, which names what flashes.
    #[prop(optional, into)]
    run: String,
    #[prop(optional_no_strip)] on_choose: Option<Callback<u64>>,
) -> impl IntoView {
    let card = NodeRef::<Button>::new();
    let number_of = issue.with_untracked(|issue| issue.number);
    flash_on(card, format!("{run}#{number_of}"));
    let class = move || {
        let state = issue.with(|issue| state_class(issue.state));
        if chosen.get() {
            format!("issue {state} chosen")
        } else {
            format!("issue {state}")
        }
    };
    let at = part(issue, |issue| place(issue.column, issue.row));
    let title = part(issue, |issue| format!("#{} {}", issue.number, issue.title));
    let name = part(issue, |issue| issue.title.clone());
    let life = part(issue, |issue| issue.life.clone());
    let figures = part(issue, |issue| issue.figures.clone());
    let corner = move || match life.get() {
        Some(life) => {
            let class = if life.starts_with("quiet") {
                "life quiet"
            } else {
                "life"
            };
            Some(view! { <span class=class>{life}</span> }.into_any())
        }
        None => figures
            .get()
            .map(|figures| view! { <span class="figures-small">{figures}</span> }.into_any()),
    };
    let choose = move |_: MouseEvent| {
        if let Some(on_choose) = on_choose {
            on_choose.run(number_of);
        }
    };
    view! {
        <button
            class=class
            style:left=move || format!("{}px", at.get().0)
            style:top=move || format!("{}px", at.get().1)
            style:width=format!("{CARD_W}px")
            style:height=format!("{CARD_H}px")
            title=title
            node_ref=card
            on:click=choose
        >
            <div class="top">
                <span class="dot"></span>
                {number(issue)}
                {corner}
            </div>
            <div class="name">{name}</div>
            {words(part(issue, |issue| issue.line.clone()), Some("card-line".into()))}
        </button>
    }
}

/// The graph's edges, each from a blocker's right side to the left side of
/// what it blocks.
#[component]
pub fn Edges(issues: Vec<IssueCard>, edges: Vec<(u64, u64)>) -> impl IntoView {
    let at = |number: u64| {
        issues
            .iter()
            .find(|issue| issue.number == number)
            .map(|issue| place(issue.column, issue.row))
    };
    edges
        .iter()
        .filter_map(|&(from, to)| Some((at(from)?, at(to)?)))
        .map(|((left, top), (right, bottom))| {
            let (x1, y1) = ((left + CARD_W) as i64, (top + CARD_H / 2) as i64);
            let (x2, y2) = (right as i64, (bottom + CARD_H / 2) as i64);
            let bend = (GAP_X as i64 / 2).max((x2 - x1) / 2);
            let d = format!(
                "M{x1},{y1} C{},{y1} {},{y2} {x2},{y2}",
                x1 + bend,
                x2 - bend
            );
            view! { <path class="edge" d=d></path> }
        })
        .collect_view()
}

/// The chosen issue's detail panel: its state and facts, and what its
/// agent is doing, in Full, the stream of its log, or in Summary, its
/// recent actions.
#[component]
pub fn Detail(
    #[prop(into)] issue: Signal<IssueCard>,
    /// The stream of its agent's log, if it has one.
    #[prop(optional, into)]
    stream: Signal<Option<Stream>>,
    /// Whether it starts on Summary rather than Full.
    #[prop(optional)]
    summary: bool,
) -> impl IntoView {
    let title = part(issue, |issue| format!(" {}", issue.title));
    let state = part(issue, |issue| issue.state);
    let worked = part(issue, |issue| {
        let name = if issue.state == NodeState::Running {
            "Working for"
        } else {
            "Took"
        };
        issue.worked.clone().map(|worked| (name, worked))
    });
    let fact = move |name: &'static str, sentence: Memo<Sentence>| {
        view! {
            <dt>{name}</dt>
            <dd>{words(sentence, None)}</dd>
        }
    };
    let summary = RwSignal::new(summary);
    let choice = move |label: &'static str, shows_summary: bool| {
        let chosen = move || summary.get() == shows_summary;
        view! {
            <button
                aria-pressed=move || if chosen() { "true" } else { "false" }
                on:click=move |_| summary.set(shows_summary)
            >
                {label}
            </button>
        }
    };
    let recent = part(issue, |issue| issue.recent.clone());
    let recent = move || {
        let recent = recent.get();
        if recent.is_empty() {
            view! { <li class="muted">"Nothing yet."</li> }.into_any()
        } else {
            recent
                .into_iter()
                .map(|action| view! { <li>{action}</li> })
                .collect_view()
                .into_any()
        }
    };
    let doing = move || {
        if summary.get() {
            view! { <ol class="recent">{recent}</ol> }.into_any()
        } else {
            view! { <StreamView stream/> }.into_any()
        }
    };
    let sentence = part(issue, |issue| issue.sentence.clone());
    view! {
        <section class="detail">
            <div class="facts">
                <h3>{number(issue)}{title}</h3>
                {move || {
                    let class = format!("state {}", state_class(state.get()));
                    view! { <Words spans=sentence.get() class/> }
                }}
                <dl>
                    {move || worked.get().map(|(name, worked)| view! { <dt>{name}</dt><dd>{worked}</dd> })}
                    {fact("Depends on", part(issue, |issue| issue.depends_on.clone()))}
                    {fact("Unblocks", part(issue, |issue| issue.unblocks.clone()))}
                    {fact("Pull request", part(issue, |issue| issue.pull_request.clone()))}
                </dl>
            </div>
            <div class="doing">
                <div class="switch" role="group" aria-label="What the agent has done">
                    {choice("Full", false)}
                    {choice("Summary", true)}
                </div>
                {doing}
            </div>
        </section>
    }
}

fn issue_number(issue: &IssueCard) -> u64 {
    issue.number
}

/// A run's page: its breadcrumb, what it is and where it stands, its
/// builds, its issues as a graph, and the chosen issue's detail panel.
#[component]
pub fn RunView(
    #[prop(into)] page: Signal<RunPage>,
    /// The issue whose detail panel shows.
    #[prop(into)]
    chosen: Signal<Option<u64>>,
    #[prop(optional)] on_choose: Option<Callback<u64>>,
    /// The stream of an agent's log, and the log's path.
    #[prop(optional, into)]
    streamed: Signal<Option<(String, Stream)>>,
) -> impl IntoView {
    let run = page.with_untracked(|page| page.run.clone());
    let also_building = part(page, |page| page.also_building.clone());
    let epic = part(page, |page| page.epic.clone());
    let about = part(page, |page| page.about.clone());
    let title = part(page, |page| page.title.clone());
    let sentence = part(page, |page| page.sentence.clone());
    let place = part(page, |page| page.place.clone());
    let disposition = part(page, |page| page.disposition);
    let elapsed = part(page, |page| page.elapsed.clone());
    let cost = part(page, |page| page.cost.clone());
    let builds = part(page, |page| page.builds.clone());
    let builds = move || {
        builds
            .get()
            .into_iter()
            .enumerate()
            .map(|(i, build)| {
                view! { <span class=if i == 0 { "build current" } else { "build" }>{build}</span> }
            })
            .collect_view()
    };
    let size = part(page, |page| {
        (
            (page.columns * (CARD_W + GAP_X)).saturating_sub(GAP_X),
            (page.rows * (CARD_H + GAP_Y)).saturating_sub(GAP_Y),
        )
    });
    let edges = part(page, |page| (page.issues.clone(), page.edges.clone()));
    let issues = part(page, |page| page.issues.clone());
    let detail = move || {
        let number = chosen.get()?;
        let issue =
            issues.with(|issues| issues.iter().find(|issue| issue.number == number).cloned())?;
        let issue = kept(issue, move || {
            issues.with(|issues| issues.iter().find(|issue| issue.number == number).cloned())
        });
        // Only the chosen issue's own stream.
        let stream = Memo::new(move |_| {
            let log = issue.with(|issue| issue.log.clone());
            streamed.with(|streamed| {
                streamed
                    .as_ref()
                    .filter(|(path, _)| log.as_ref() == Some(path))
                    .map(|(_, stream)| stream.clone())
            })
        });
        Some(view! { <Detail issue stream/> })
    };
    view! {
        <nav class="run-bar"><RunBar title also_building/></nav>
        <section class="run-head">
            <div>
                <div class="about">{link(epic)}" · "{words(about, None)}</div>
                <h1>{title}</h1>
                <p class=move || format!("sentence {}", disposition_class(disposition.get()))>{sentence}</p>
                {move || place.get().map(|place| view! { <p class="place">{place}</p> })}
            </div>
            <dl class="totals">
                <dt>"Elapsed"</dt>
                <dd>{elapsed}</dd>
                <dt>"Cost so far"</dt>
                <dd>{cost}</dd>
            </dl>
        </section>
        <div class="builds">
            <span class="label">"Builds"</span>
            {builds}
        </div>
        <div class="graph-scroll">
            <div
                class="graph"
                style:width=move || format!("{}px", size.get().0)
                style:height=move || format!("{}px", size.get().1)
            >
                <svg width=move || size.get().0 height=move || size.get().1>
                    {move || {
                        let (issues, edges) = edges.get();
                        view! { <Edges issues edges/> }
                    }}
                </svg>
                {keyed(issues.into(), issue_number, move |issue| {
                    let number = issue.with_untracked(issue_number);
                    let chosen = Signal::derive(move || chosen.get() == Some(number));
                    view! { <Issue issue chosen run=run.clone() on_choose/> }
                })}
            </div>
        </div>
        {detail}
    }
}
