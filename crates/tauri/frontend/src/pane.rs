//! The tab strip and the feature pane: the monitor, drawn.
//!
//! Both draw what the [`View`] fold decides and nothing else: a
//! [`TabSpec`] per build, a [`Pane`] for the open one. The pane is the
//! plan as the library lays it out — containment as labelled rounded
//! rects, ordering as arrows, left to right — scaled by [`geometry`]
//! into an SVG whose `viewBox` fits the picture, with a slim column
//! naming the selected issue beside it.
//!
//! Fill means state and nothing else; selection is a ring. The graph
//! palette is the `.graph-*` classes in `styles.css`, taken from
//! `brand.json`'s `graph` group and stated once per theme there, so no
//! colour appears in this markup; a node paints itself `currentColor`.
//! Motion is the one `epik-pulse` keyframe, on a Running node and on
//! the dot of a tab with one — and only while the feed is speaking.

use epik_core::monitor::RunId;
use epik_core::monitor::layout::{Form, Layout};
use leptos::prelude::*;

use crate::monitor::{Dot, Pane, Tab, TabSpec, View};

/// The grid, in SVG units: a column's stride, a row's.
const COLUMN: f64 = 150.0;
const ROW: f64 = 88.0;
/// A node's radius, settled and in flight.
const R: f64 = 17.0;
const R_ACTIVE: f64 = 19.0;
/// How far a box stands off the nodes in it, per nesting level.
const NEST: f64 = 12.0;

/// One node, placed.
#[derive(Clone, Debug, PartialEq)]
pub struct Circle {
    pub id: u64,
    pub cx: f64,
    pub cy: f64,
    pub r: f64,
}

/// One container's box.
#[derive(Clone, Debug, PartialEq)]
pub struct Rect {
    pub label: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One arrow, blocker to issue, as an SVG path.
#[derive(Clone, Debug, PartialEq)]
pub struct Arrow {
    pub d: String,
}

/// The picture in SVG units: the `viewBox` and everything in it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Geometry {
    pub width: f64,
    pub height: f64,
    pub circles: Vec<Circle>,
    pub rects: Vec<Rect>,
    pub arrows: Vec<Arrow>,
}

/// Scales a layout onto the grid. Pure, so the `viewBox` is pinned by a
/// test rather than by looking.
#[must_use]
pub fn geometry(layout: &Layout) -> Geometry {
    let nesting = layout
        .containers
        .iter()
        .map(|container| container.height + 1)
        .max()
        .unwrap_or(0);
    let inset = NEST * nesting as f64;
    let left = 56.0 + inset;
    let top = 36.0 + inset;
    let bottom = 52.0 + inset;
    let at = |column: usize, row: usize| (left + column as f64 * COLUMN, top + row as f64 * ROW);

    let circles: Vec<Circle> = layout
        .nodes
        .iter()
        .map(|node| {
            let (cx, cy) = at(node.column, node.row);
            Circle {
                id: node.id,
                cx,
                cy,
                r: match node.form {
                    Form::Running | Form::Merging => R_ACTIVE,
                    _ => R,
                },
            }
        })
        .collect();
    let circle = |id: u64| circles.iter().find(|circle| circle.id == id);

    let rects = layout
        .containers
        .iter()
        .map(|container| {
            let inside: Vec<&Circle> = container
                .leaves
                .iter()
                .copied()
                .filter_map(circle)
                .collect();
            let pad = NEST * container.height as f64;
            let x = inside.iter().map(|c| c.cx).fold(f64::MAX, f64::min) - R_ACTIVE - 10.0 - pad;
            let right =
                inside.iter().map(|c| c.cx).fold(f64::MIN, f64::max) + R_ACTIVE + 10.0 + pad;
            let y = inside.iter().map(|c| c.cy).fold(f64::MAX, f64::min) - R_ACTIVE - 8.0 - pad;
            let below =
                inside.iter().map(|c| c.cy).fold(f64::MIN, f64::max) + R_ACTIVE + 22.0 + pad;
            Rect {
                label: format!("#{} {}", container.id, container.title),
                x,
                y,
                width: right - x,
                height: below - y,
            }
        })
        .collect();

    let arrows = layout
        .edges
        .iter()
        .filter_map(|edge| Some((circle(edge.blocker)?, circle(edge.issue)?)))
        .map(|(from, to)| {
            let (x1, y1) = (from.cx + from.r, from.cy);
            let (x2, y2) = (to.cx - to.r - 4.0, to.cy);
            let mid = f64::midpoint(x1, x2);
            Arrow {
                d: format!("M{x1:.1} {y1:.1} C{mid:.1} {y1:.1}, {mid:.1} {y2:.1}, {x2:.1} {y2:.1}"),
            }
        })
        .collect();

    let (columns, rows) = (layout.columns(), layout.rows());
    Geometry {
        width: if columns == 0 {
            2.0 * left
        } else {
            left * 2.0 + (columns - 1) as f64 * COLUMN
        },
        height: if rows == 0 {
            top + bottom
        } else {
            top + bottom + (rows - 1) as f64 * ROW
        },
        circles,
        rects,
        arrows,
    }
}

/// What every tab wears, and what the open one adds.
const TAB: &str = "flex cursor-pointer items-center gap-2 px-4 py-2.5 text-[13px] select-none";
const ACTIVE: &str = "-mb-px rounded-t-md border border-b-2 border-neutral-200 border-b-[#00b377] \
                      bg-neutral-50 font-medium text-neutral-900 \
                      dark:border-neutral-800 dark:border-b-[#00e599] dark:bg-neutral-900 dark:text-neutral-100";
const INACTIVE: &str =
    "text-neutral-500 hover:text-neutral-900 dark:text-neutral-400 dark:hover:text-neutral-100";

/// The class a dot wears for its colour, and whether it is hollow.
const fn dot_class(dot: Dot) -> &'static str {
    match dot {
        Dot::Error => "graph-error bg-current",
        Dot::Open => "graph-open bg-current",
        Dot::Closed => "graph-closed bg-current",
        Dot::Muted => "graph-strong border-2 border-current",
    }
}

/// The dot itself, wherever one is drawn: a tab's, and the one beside
/// the selected issue's sentence. Stated once, so the two agree.
const DOT: &str = "inline-block h-[7px] w-[7px] shrink-0 rounded-full";

/// What a tab's dot wears altogether: its size, its colour, and its
/// motion when there is an Agent working and the feed is speaking.
fn dot_classes(dot: Dot, pulse: bool) -> String {
    format!(
        "{DOT} {}{}",
        dot_class(dot),
        if pulse { " epik-pulse" } else { "" }
    )
}

/// The class a form wears: its colour, and its motion when the feed is
/// speaking.
const fn form_class(form: Form, pulse: bool) -> &'static str {
    match (form, pulse) {
        (Form::Blocked | Form::Skipped, _) => "graph-strong",
        (Form::Ready | Form::Merging, _) | (Form::Running, false) => "graph-open",
        (Form::Running, true) => "graph-open epik-pulse",
        (Form::Merged, _) => "graph-closed",
        (Form::Failed, _) => "graph-error",
    }
}

/// The strip: the assistant first, where there is one, then a tab per build
/// the fold knows.
#[component]
pub fn TabStrip(view: RwSignal<View>) -> impl IntoView {
    let assistant = view.with_untracked(View::has_assistant).then_some(move || {
        let active = view.with(|view| view.tab() == Tab::Assistant);
        view! {
            <div
                role="tab"
                class=format!("{TAB} {}", if active { ACTIVE } else { INACTIVE })
                on:click=move |_| view.update(|view| view.open(Tab::Assistant))
            >
                "Epik"
            </div>
        }
    });
    let features = move || {
        view.with(View::tabs)
            .into_iter()
            .map(|tab| feature_tab(view, tab))
            .collect_view()
    };
    view! {
        <nav
            role="tablist"
            class="flex shrink-0 items-stretch gap-0.5 border-b border-neutral-200 bg-neutral-100 px-3 dark:border-neutral-800 dark:bg-neutral-950"
        >
            {assistant}
            {features}
        </nav>
    }
}

fn feature_tab(view: RwSignal<View>, tab: TabSpec) -> impl IntoView {
    let run = tab.run;
    let dot = dot_classes(tab.dot, tab.pulse);
    let close = tab.closable.then(|| {
        view! {
            <button
                type="button"
                aria-label="Close"
                class="-mr-1.5 ml-0.5 rounded px-1 text-neutral-400 hover:bg-neutral-200 hover:text-neutral-700 dark:hover:bg-neutral-800 dark:hover:text-neutral-200"
                on:click=move |event| {
                    event.stop_propagation();
                    view.update(|view| view.close(run));
                }
            >
                "×"
            </button>
        }
    });
    view! {
        <div
            role="tab"
            class=format!("{TAB} {}", if tab.active { ACTIVE } else { INACTIVE })
            on:click=move |_| view.update(|view| view.open(Tab::Feature(run)))
        >
            <span class=dot></span>
            <span class="max-w-[24ch] truncate">{tab.label}</span>
            {tab.count.map(|count| view! {
                <span class="font-mono text-[11px] text-neutral-500 dark:text-neutral-400">{count}</span>
            })}
            {close}
        </div>
    }
}

/// What a small caps label in the detail column wears.
const LABEL: &str =
    "font-mono text-[10.5px] tracking-[0.06em] text-neutral-400 uppercase dark:text-neutral-500";
/// What a notice line wears.
const NOTICE: &str = "mx-6 mt-1 rounded-lg border px-3.5 py-2 text-[13px]";

/// One feature's pane: the graph, and the selected issue beside it.
#[component]
pub fn FeaturePane(view: RwSignal<View>, pane: Pane) -> impl IntoView {
    let run = pane.run;
    let disconnected = pane.disconnected.map(|why| {
        view! {
            <div class=format!(
                "{NOTICE} border-neutral-300 bg-neutral-100 text-neutral-600 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-300"
            )>
                "Not connected — "{why}
            </div>
        }
    });
    let notices = pane
        .notices
        .into_iter()
        .map(|notice| {
            view! {
                <div class=format!(
                    "{NOTICE} border-[#dc2626]/30 bg-[#dc2626]/5 text-[#dc2626] dark:border-[#ef4444]/30 dark:bg-[#ef4444]/10 dark:text-[#ef4444]"
                )>
                    {notice}
                </div>
            }
        })
        .collect_view();
    let graph = pane
        .layout
        .as_ref()
        .map(|layout| graph(view, run, layout, pane.selected, pane.pulse));
    let detail = pane.detail.map(|detail| {
        let colour = form_class(detail.form, false);
        view! {
            <div class="text-[15px] leading-[1.35] font-medium text-neutral-900 dark:text-neutral-100">
                {detail.heading}
            </div>
            <div class=format!("flex items-start gap-2 text-[13px] {colour}")>
                <span class=format!("mt-[6px] {DOT} bg-current")></span>
                <span class="break-words">{detail.sentence}</span>
            </div>
            <div class=format!("{LABEL} mt-2 border-t border-neutral-200 pt-2.5 dark:border-neutral-800")>"Blocks"</div>
            <div class="font-mono text-[12px] text-neutral-600 dark:text-neutral-300">{detail.blocks}</div>
        }
    });
    view! {
        <section class="flex min-h-0 flex-1">
            <div class="flex min-w-0 flex-1 flex-col">
                <div class="flex items-baseline justify-between gap-4 px-6 pt-4 pb-1.5">
                    <div class="flex min-w-0 items-baseline gap-3">
                        <h1 class="truncate text-[17px] font-medium tracking-tight text-neutral-900 dark:text-neutral-100">
                            {pane.heading}
                        </h1>
                        <span class="truncate font-mono text-[11px] text-neutral-500 dark:text-neutral-400">
                            {pane.origin}
                        </span>
                    </div>
                    <span class="shrink-0 font-mono text-[11px] text-neutral-500 dark:text-neutral-400">
                        {pane.counts}
                    </span>
                </div>
                {disconnected}
                {notices}
                <div class="min-h-0 flex-1 px-6 pt-2.5 pb-5">
                    <div class="h-full rounded-lg border border-neutral-200 bg-neutral-100 p-2 dark:border-neutral-800 dark:bg-neutral-950">
                        {graph}
                    </div>
                </div>
            </div>
            <aside class="flex w-72 shrink-0 flex-col gap-2.5 border-l border-neutral-200 bg-neutral-100 px-4 py-4 dark:border-neutral-800 dark:bg-neutral-950">
                <div class=LABEL>"Selected issue"</div>
                {match detail {
                    Some(detail) => detail.into_any(),
                    None => view! {
                        <div class="text-[13px] text-neutral-500 dark:text-neutral-400">
                            "Click a node to see where it stands."
                        </div>
                    }
                        .into_any(),
                }}
            </aside>
        </section>
    }
}

/// The graph: boxes under arrows under nodes, the selected node ringed.
fn graph(
    view: RwSignal<View>,
    run: RunId,
    layout: &Layout,
    selected: Option<u64>,
    pulse: bool,
) -> impl IntoView + use<> {
    let geometry = geometry(layout);
    let rects = geometry
        .rects
        .iter()
        .map(|rect| {
            view! {
                <rect
                    x=rect.x
                    y=rect.y
                    width=rect.width
                    height=rect.height
                    rx="10"
                    fill="none"
                    class="stroke-neutral-300 dark:stroke-neutral-700"
                    stroke-width="1.5"
                />
                <text
                    x=rect.x + 8.0
                    y=rect.y - 6.0
                    class="fill-neutral-500 font-mono text-[11px] dark:fill-neutral-400"
                >
                    {rect.label.clone()}
                </text>
            }
        })
        .collect_view();
    let arrows = geometry
        .arrows
        .iter()
        .map(|arrow| {
            view! {
                <path
                    d=arrow.d.clone()
                    class="graph-link"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    marker-end="url(#epik-arrow)"
                />
            }
        })
        .collect_view();
    let nodes = layout
        .nodes
        .iter()
        .zip(&geometry.circles)
        .map(|(node, circle)| {
            let (cx, cy, r) = (circle.cx, circle.cy, circle.r);
            let id = node.id;
            let colour = form_class(node.form, pulse);
            let ring = selected.is_some_and(|selected| selected == node.id).then(|| {
                view! {
                    <circle
                        cx=cx
                        cy=cy
                        r=r + 9.0
                        fill="none"
                        class="stroke-neutral-900 dark:stroke-neutral-100"
                        stroke-width="2"
                    />
                }
            });
            // Merging is ringed in the closed colour; Running keeps a halo
            // where motion is turned off, so the state still reads.
            let halo = match node.form {
                Form::Merging => Some(view! {
                    <circle cx=cx cy=cy r=r + 5.0 fill="none" class="graph-closed" stroke="currentColor" stroke-width="2.5" />
                }),
                Form::Running => Some(view! {
                    <circle cx=cx cy=cy r=r + 5.0 fill="none" class="graph-open graph-halo" stroke="currentColor" stroke-width="2.5" />
                }),
                _ => None,
            };
            let body = match node.form {
                Form::Blocked => view! {
                    <circle cx=cx cy=cy r=r fill="none" class=colour stroke="currentColor" stroke-width="2.5" />
                }
                .into_any(),
                Form::Ready => view! {
                    <circle cx=cx cy=cy r=r fill="none" class=colour stroke="currentColor" stroke-width="2.5" stroke-dasharray="6 4" />
                }
                .into_any(),
                Form::Skipped => view! {
                    <circle cx=cx cy=cy r=r fill="none" class=colour stroke="currentColor" stroke-width="2" stroke-dasharray="3 4" opacity="0.5" />
                }
                .into_any(),
                Form::Running | Form::Merging | Form::Merged | Form::Failed => view! {
                    <circle cx=cx cy=cy r=r class=colour fill="currentColor" />
                }
                .into_any(),
            };
            view! {
                <g
                    class="cursor-pointer"
                    on:click=move |_| view.update(|view| view.select(run, id))
                >
                    <title>{format!("#{} {}", node.id, node.title)}</title>
                    {ring}
                    {halo}
                    {body}
                    <text
                        x=cx
                        y=cy + r + 15.0
                        text-anchor="middle"
                        class="fill-neutral-600 font-mono text-[12px] dark:fill-neutral-300"
                    >
                        {format!("#{}", node.id)}
                    </text>
                </g>
            }
        })
        .collect_view();
    view! {
        <svg
            class="h-full w-full"
            viewBox=format!("0 0 {:.0} {:.0}", geometry.width, geometry.height)
            preserveAspectRatio="xMidYMid meet"
        >
            <defs>
                <marker
                    id="epik-arrow"
                    viewBox="0 0 10 10"
                    refX="9"
                    refY="5"
                    markerWidth="5"
                    markerHeight="5"
                    orient="auto-start-reverse"
                >
                    <path d="M0 0 L10 5 L0 10 z" class="graph-link" fill="currentColor" />
                </marker>
            </defs>
            {rects}
            {arrows}
            {nodes}
        </svg>
    }
}

#[cfg(test)]
mod tests {
    use epik_core::monitor::State;
    use epik_core::monitor::layout::{Container, Edge, Node};

    use super::*;

    fn node(id: u64, column: usize, row: usize, form: Form) -> Node {
        Node {
            id,
            title: format!("issue {id}"),
            state: State::Waiting,
            form,
            column,
            row,
            blocked_by: Vec::new(),
            blocks: Vec::new(),
        }
    }

    #[test]
    fn the_view_box_fits_the_grid_and_in_flight_nodes_are_larger() {
        let layout = Layout {
            nodes: vec![node(2, 0, 0, Form::Running), node(3, 1, 1, Form::Blocked)],
            edges: vec![Edge {
                blocker: 2,
                issue: 3,
            }],
            containers: Vec::new(),
        };
        let geometry = geometry(&layout);
        assert_eq!((geometry.width, geometry.height), (262.0, 176.0));
        assert_eq!(geometry.circles[0].r, R_ACTIVE);
        assert_eq!(geometry.circles[1].r, R);
        assert_eq!(
            (geometry.circles[1].cx, geometry.circles[1].cy),
            (206.0, 124.0)
        );
        assert_eq!(geometry.arrows.len(), 1);
        assert!(geometry.arrows[0].d.starts_with("M75.0 36.0 C"));
    }

    #[test]
    fn a_box_stands_off_its_nodes_and_the_margins_make_room_for_it() {
        let layout = Layout {
            nodes: vec![node(3, 0, 0, Form::Ready), node(4, 0, 1, Form::Ready)],
            edges: Vec::new(),
            containers: vec![Container {
                id: 7,
                title: "the container".to_owned(),
                depth: 1,
                height: 0,
                leaves: vec![3, 4],
            }],
        };
        let geometry = geometry(&layout);
        assert_eq!(geometry.rects.len(), 1);
        let rect = &geometry.rects[0];
        assert_eq!(rect.label, "#7 the container");
        let (cx, cy) = (geometry.circles[0].cx, geometry.circles[0].cy);
        assert!(
            rect.x < cx - R && rect.y < cy - R,
            "the box is outside the node"
        );
        assert!(rect.x > 0.0 && rect.y > 0.0, "and inside the picture");
        assert!(rect.x + rect.width < geometry.width);
        assert!(rect.y + rect.height < geometry.height);
    }

    #[test]
    fn an_empty_layout_still_has_a_view_box() {
        let geometry = geometry(&Layout::default());
        assert!(geometry.width > 0.0 && geometry.height > 0.0);
    }

    /// The palette class and the fill: a muted dot is hollow, and the
    /// three that stand for a state are filled.
    #[test]
    fn a_dot_carries_its_colour_from_the_graph_palette() {
        assert_eq!(dot_class(Dot::Error), "graph-error bg-current");
        assert_eq!(dot_class(Dot::Open), "graph-open bg-current");
        assert_eq!(dot_class(Dot::Closed), "graph-closed bg-current");
        assert_eq!(
            dot_class(Dot::Muted),
            "graph-strong border-2 border-current"
        );
    }

    #[test]
    fn every_dot_is_the_one_dot_wearing_its_own_colour_and_pulsing_on_request() {
        for dot in [Dot::Error, Dot::Open, Dot::Closed, Dot::Muted] {
            let still = dot_classes(dot, false);
            assert!(still.contains(DOT), "the shared geometry: {still}");
            assert!(still.contains(dot_class(dot)), "and its own: {still}");
            assert!(!still.contains("epik-pulse"), "{dot:?}");
            assert!(dot_classes(dot, true).contains("epik-pulse"), "{dot:?}");
        }
    }

    /// Every form at both flags: the colour it wears, and the one that
    /// moves. Running is the only row whose two entries differ, so the
    /// table says what pulses by showing what does not.
    #[test]
    fn a_form_wears_the_colour_its_state_calls_for_and_only_running_moves() {
        let table = [
            (Form::Blocked, "graph-strong", "graph-strong"),
            (Form::Ready, "graph-open", "graph-open"),
            (Form::Skipped, "graph-strong", "graph-strong"),
            (Form::Running, "graph-open", "graph-open epik-pulse"),
            (Form::Merging, "graph-open", "graph-open"),
            (Form::Merged, "graph-closed", "graph-closed"),
            (Form::Failed, "graph-error", "graph-error"),
        ];
        for (form, still, speaking) in table {
            assert_eq!(form_class(form, false), still, "{form:?}");
            assert_eq!(form_class(form, true), speaking, "{form:?}");
        }
    }

    #[test]
    fn an_edge_naming_a_node_the_layout_lacks_draws_nothing() {
        let layout = Layout {
            nodes: vec![node(1, 0, 0, Form::Ready)],
            edges: vec![
                Edge {
                    blocker: 1,
                    issue: 9,
                },
                Edge {
                    blocker: 9,
                    issue: 1,
                },
            ],
            containers: Vec::new(),
        };
        assert_eq!(geometry(&layout).arrows, []);
    }

    #[test]
    fn a_nested_box_stands_further_off_than_the_one_inside_it() {
        let inner = |height| Layout {
            nodes: vec![node(3, 0, 0, Form::Ready), node(4, 1, 0, Form::Ready)],
            edges: Vec::new(),
            containers: vec![Container {
                id: 7,
                title: "the container".to_owned(),
                depth: 1,
                height,
                leaves: vec![3, 4],
            }],
        };
        let flat = geometry(&inner(0));
        let nested = geometry(&inner(1));
        assert!(nested.rects[0].width > flat.rects[0].width);
        assert!(
            nested.width > flat.width && nested.height > flat.height,
            "the margins make room for the deeper nesting"
        );
    }

    /// The layout gives every node its own row, so a chain down the
    /// diagonal is the shape a real one has.
    #[test]
    fn the_view_box_grows_by_a_stride_per_node() {
        let chain = |length: usize| Layout {
            nodes: (0..length)
                .map(|step| node(step as u64, step, step, Form::Ready))
                .collect(),
            edges: Vec::new(),
            containers: Vec::new(),
        };
        let (one, three) = (geometry(&chain(1)), geometry(&chain(3)));
        assert_eq!(three.width - one.width, 2.0 * COLUMN);
        assert_eq!(three.height - one.height, 2.0 * ROW);
    }
}
