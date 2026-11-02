//! The main window: a sidebar beside Home (`#/`), a run's page
//! (`#/run/<owner/name%23n>`, or `#/run/<host>:<owner/name%23n>` for a
//! run on a remote host), or Settings (`#/settings`). It asks the app
//! what to show every second and draws it. Its only controls are Rerun and
//! Delete on a run that needs attention, and Settings, which also chooses
//! each role's agent and model, adds, checks and removes hosts, and can
//! uninstall Epik.

use std::collections::HashMap;
use std::time::Duration;

use epik_view::{
    AgentSettings, Disposition, Home, HostRow, NodeState, RepositoryRow, RunPage, Settings,
    Sidebar as SidebarView, SidebarRun, Stream, Uninstall, UninstallStep,
};
use leptos::ev::MouseEvent;
use leptos::html::{Button, Dialog};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;

use crate::flash::{Flashes, Flashing};
use crate::home::{Act, HomePage};
use crate::live::{kept, keyed, part};
use crate::run_page::{RunView, shown_log};
use crate::settings::{
    ConfirmBody, ConfirmHostBody, HostActions, SettingsPage, UninstallBody, Uninstalling,
};
use crate::tauri;

const REFRESH: Duration = Duration::from_secs(1);

/// A page of the main window, as its URL's hash names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Page {
    /// `#/`, and any hash that isn't another page's.
    Home,
    /// `#/run/<owner/name%23n>`, after `<host>:` for a run on a remote
    /// host: the run's page.
    Run(String),
    /// `#/settings`.
    Settings,
}

impl Page {
    pub fn from_hash(hash: &str) -> Page {
        let path = hash.strip_prefix('#').unwrap_or(hash);
        let path = decode(path.strip_prefix('/').unwrap_or(path));
        match path.strip_prefix("run/") {
            Some(run) => Page::Run(run.to_owned()),
            None if path == "settings" => Page::Settings,
            None => Page::Home,
        }
    }
}

/// `text` with its `%XX` escapes decoded.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%')
            .then(|| text.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                decoded.push(byte);
                i += 3;
            }
            None => {
                decoded.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// The hash for `path`: `""` for Home, `run/<owner/name#n>` for a run's
/// page (after `<host>:` on a remote host), `settings` for Settings. A run's `#` would end the URL's
/// fragment, so it is escaped.
pub fn href(path: &str) -> String {
    format!("#/{}", path.replace('#', "%23"))
}

/// Epik's mark, set as the span's HTML: Leptos would wrap the static SVG's
/// children in groups.
const MARK: &str = r##"<svg viewBox="0 0 64 64" width="20" height="20" aria-hidden="true"><g stroke="#171717" stroke-width="3" opacity="0.3"><line x1="16" y1="16" x2="48" y2="24"/><line x1="16" y1="16" x2="24" y2="48"/><line x1="48" y1="24" x2="40" y2="48"/><line x1="24" y1="48" x2="40" y2="48"/></g><circle cx="16" cy="16" r="7" fill="#00c07a"/><circle cx="48" cy="24" r="6" fill="#171717"/><circle cx="24" cy="48" r="6" fill="#171717"/><circle cx="40" cy="48" r="5" fill="#00c07a" opacity="0.7"/></svg>"##;

/// A run's status dot's class.
pub(crate) fn disposition_class(disposition: Disposition) -> &'static str {
    match disposition {
        Disposition::Building | Disposition::Reviewing => "building",
        Disposition::WaitingForReview => "waiting_for_review",
        Disposition::NeedsAttention => "needs_attention",
    }
}

fn sidebar_key(run: &SidebarRun) -> String {
    run.run.clone()
}

/// The sidebar beside every page: the mark and name; Runs, which opens
/// Home, with a foldable list of the runs, each opening its page; and at
/// the bottom Settings, which opens its page, marked when a monitored
/// repository is no longer on GitHub, and how long Epik has been running.
/// The current page is highlighted.
#[component]
pub fn Sidebar(
    #[prop(into)] sidebar: Signal<SidebarView>,
    #[prop(into)] page: Signal<Page>,
    #[prop(optional)] folded: bool,
) -> impl IntoView {
    let folded = RwSignal::new(folded);
    let runs = part(sidebar, |sidebar| sidebar.runs.clone());
    let count = part(sidebar, |sidebar| sidebar.runs.len());
    let missing = part(sidebar, |sidebar| sidebar.missing_repository);
    let status = part(sidebar, |sidebar| sidebar.status.clone());
    let at_home = Memo::new(move |_| page.with(|page| *page == Page::Home));
    let at_settings = Memo::new(move |_| page.with(|page| *page == Page::Settings));
    let fold = move |_: MouseEvent| folded.update(|folded| *folded = !*folded);
    let run_item = move |run: Signal<SidebarRun>| {
        let key = part(run, |run| run.run.clone());
        let class = Memo::new(move |_| {
            let current = page.with(|page| matches!(page, Page::Run(at) if *at == key.get()));
            if current {
                "item run current"
            } else {
                "item run"
            }
        });
        let link = part(run, |run| href(&format!("run/{}", run.run)));
        let dot = part(run, |run| {
            format!("dot {}", disposition_class(run.disposition))
        });
        let title = part(run, |run| run.title.clone());
        view! {
            <li>
                <a href=link class=class>
                    <span class=dot></span>
                    <span class="title">{title}</span>
                </a>
            </li>
        }
    };
    view! {
        <div class="brand">
            <span class="mark" inner_html=MARK></span>
            <span class="name">"Epik"</span>
        </div>
        <div class=move || if at_home.get() { "item runs current" } else { "item runs" }>
            <button
                aria-label=move || if folded.get() { "Unfold the runs" } else { "Fold the runs" }
                aria-expanded=move || if folded.get() { "false" } else { "true" }
                class="fold"
                on:click=fold
            >
                {move || if folded.get() { "▸" } else { "▾" }}
            </button>
            <a href="#/" class="label">"Runs"</a>
            <span class="count">{count}</span>
        </div>
        {move || {
            (!folded.get())
                .then(|| view! { <ul class="run-list">{keyed(runs.into(), sidebar_key, run_item)}</ul> })
        }}
        <div class="foot">
            <a
                href=href("settings")
                class=move || if at_settings.get() { "item settings current" } else { "item settings" }
            >
                "Settings"
                {move || missing.get().then(|| view! { <span class="badge">"✕"</span> })}
            </a>
            <div class="status">
                <span class="dot"></span>
                {status}
            </div>
        </div>
    }
}

/// The breadcrumb at the top of a run's page: a link back to Home, the
/// run, and links to the other runs building.
#[component]
pub fn RunBar(
    #[prop(into)] title: Signal<String>,
    #[prop(into)] also_building: Signal<Vec<(String, String)>>,
) -> impl IntoView {
    let also = move || {
        let also_building = also_building.get();
        (!also_building.is_empty()).then(|| {
            let links = also_building
                .into_iter()
                .enumerate()
                .map(|(i, (run, title))| {
                    view! {
                        {(i > 0).then_some(", ")}
                        <a href=href(&format!("run/{run}"))>{title}</a>
                    }
                })
                .collect_view();
            view! { <span class="also">"Also building: "{links}</span> }
        })
    };
    view! {
        <span class="crumbs">
            <a class="back" href="#/">"‹ Runs"</a>
            <span class="slash">"/"</span>
            <span>{title}</span>
        </span>
        {also}
    }
}

/// What a refresh fetched, or why it couldn't.
#[derive(Debug, Clone)]
enum Shown {
    Home(Home),
    Run(Box<RunPage>),
    /// Settings, or why they couldn't be read, which the page shows in
    /// place of the repositories.
    Settings(Result<Settings, String>),
    Failed(String),
}

/// Which page the window draws. The page's nodes stay while it doesn't
/// change.
#[derive(Debug, Clone, PartialEq)]
enum Which {
    Nothing,
    Home,
    Run(String),
    Settings,
    Failed(String),
}

/// The keys of what flashes in `shown`: each running issue whose agent
/// logged since it was last seen.
fn flashed(flashes: &mut Flashes, shown: &Shown) -> Vec<String> {
    let seen: Vec<(String, Option<i64>)> = match shown {
        Shown::Home(home) => home
            .building
            .iter()
            .flat_map(|card| {
                card.now
                    .iter()
                    .map(|now| (format!("{}#{}", card.run, now.number), now.last_event))
            })
            .collect(),
        Shown::Run(page) => page
            .issues
            .iter()
            .map(|issue| (format!("{}#{}", page.run, issue.number), issue.last_event))
            .collect(),
        Shown::Settings(_) | Shown::Failed(_) => vec![],
    };
    seen.into_iter()
        .filter(|(key, last_event)| flashes.changed(key, *last_event))
        .map(|(key, _)| key)
        .collect()
}

#[derive(Serialize)]
struct Run {
    run: String,
}

#[derive(Serialize)]
struct Repo {
    repo: String,
}

#[derive(Serialize)]
struct Destination {
    destination: Option<String>,
}

/// What the confirmation dialog on the Settings page asks about.
#[derive(Debug, Clone)]
enum Confirming {
    /// No longer monitoring a repository.
    Repository(RepositoryRow),
    /// Removing a host.
    Host(HostRow),
}

#[derive(Serialize)]
struct Log {
    path: String,
}

/// The issue whose detail panel shows on a run's page: the one picked, or
/// else the first running, or else the first.
fn chosen_issue(page: &RunPage, picked: Option<u64>) -> Option<u64> {
    picked.or_else(|| {
        page.issues
            .iter()
            .find(|issue| issue.state == NodeState::Running)
            .or(page.issues.first())
            .map(|issue| issue.number)
    })
}

/// What reading the shown session's agent log on a run's page found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    /// There is no session to show.
    NoLog,
    /// The stream in the log at the path.
    Read(String, Stream),
    /// The log at the path, which couldn't be read this time.
    Unread(String),
}

/// The stream of the shown session's agent log on a run's page, with
/// issue `picked` and the session whose log is `session` picked.
async fn fetch_stream(page: &RunPage, picked: Option<u64>, session: Option<&str>) -> Fetched {
    let Some(path) = shown_log(page, chosen_issue(page, picked), session) else {
        return Fetched::NoLog;
    };
    match tauri::invoke::<_, Stream>("agent_stream", &Log { path: path.clone() }).await {
        Ok(stream) => Fetched::Read(path, stream),
        Err(_) => Fetched::Unread(path),
    }
}

/// The stream, and its log's path, to show after `before` once a refresh
/// has `fetched`: a log that couldn't be read this time keeps the stream it
/// had.
pub fn next_stream(before: Option<(String, Stream)>, fetched: Fetched) -> Option<(String, Stream)> {
    match fetched {
        Fetched::NoLog => None,
        Fetched::Read(path, stream) => Some((path, stream)),
        Fetched::Unread(path) => before.filter(|(shown, _)| *shown == path),
    }
}

fn current_page() -> Page {
    Page::from_hash(&window().location().hash().unwrap_or_default())
}

/// Fetches what the confirmation to uninstall Epik says, or shows why it
/// couldn't.
async fn fetch_uninstall_plan(plan: RwSignal<Uninstall>, stage: RwSignal<Uninstalling>) {
    match tauri::invoke::<_, Uninstall>("uninstall_plan", &()).await {
        Ok(fetched) => plan.set(fetched),
        Err(reason) => stage.set(Uninstalling::Asking(Some(reason))),
    }
}

/// The main window, refreshed from the app every second.
#[component]
pub fn MainWindow() -> impl IntoView {
    let page = RwSignal::new(current_page());
    let shown = RwSignal::new(None::<Shown>);
    // The sidebar, which keeps what it last showed when a refresh fails.
    let sidebar = RwSignal::new(SidebarView {
        runs: vec![],
        missing_repository: false,
        status: String::new(),
    });
    // What the user asked for that failed, until the page changes.
    let error = RwSignal::new(String::new());
    // The issue chosen on each run's page.
    let selected = RwSignal::new(HashMap::<String, u64>::new());
    // The log of the session picked on each run's page.
    let picked_logs = RwSignal::new(HashMap::<String, String>::new());
    // The stream of the chosen issue's agent log, and the log's path.
    let streamed = RwSignal::new(None::<(String, Stream)>);
    let flashing = Flashing(RwSignal::new(HashMap::new()));
    provide_context(flashing);
    let flashes = StoredValue::new(Flashes::default());
    let refreshing = StoredValue::new(false);

    let refresh = move || {
        if refreshing.get_value() {
            return;
        }
        refreshing.set_value(true);
        spawn_local(async move {
            if let Ok(fetched) = tauri::invoke::<_, SidebarView>("sidebar", &()).await {
                sidebar.set(fetched);
            }
            let at = page.get_untracked();
            let fetched = match &at {
                Page::Home => tauri::invoke::<_, Home>("home", &()).await.map(Shown::Home),
                Page::Run(run) => {
                    tauri::invoke::<_, RunPage>("run_page", &Run { run: run.clone() })
                        .await
                        .map(|page| Shown::Run(Box::new(page)))
                }
                Page::Settings => Ok(Shown::Settings(
                    tauri::invoke::<_, Settings>("settings", &()).await,
                )),
            };
            match fetched {
                Ok(fetched) => {
                    let stream = match &fetched {
                        Shown::Run(page) => {
                            let picked = selected
                                .with_untracked(|selected| selected.get(&page.run).copied());
                            let session =
                                picked_logs.with_untracked(|picked| picked.get(&page.run).cloned());
                            fetch_stream(page, picked, session.as_deref()).await
                        }
                        _ => Fetched::NoLog,
                    };
                    streamed.set(next_stream(streamed.get_untracked(), stream));
                    let title = match &fetched {
                        Shown::Run(page) => format!("{} · Epik", page.title),
                        Shown::Settings(_) => "Settings · Epik".to_owned(),
                        _ => "Epik".to_owned(),
                    };
                    document().set_title(&title);
                    let keys = flashes
                        .try_update_value(|flashes| flashed(flashes, &fetched))
                        .unwrap_or_default();
                    flashing.flash(keys);
                    shown.set(Some(fetched));
                }
                Err(reason) if matches!(at, Page::Run(_)) && reason.starts_with("no run") => {
                    let _ = window().location().set_hash(&href(""));
                }
                Err(reason) => shown.set(Some(Shown::Failed(format!(
                    "Couldn't read the runs: {reason}"
                )))),
            }
            refreshing.set_value(false);
        });
    };
    refresh();
    set_interval(refresh, REFRESH);
    let _ = window_event_listener(leptos::ev::hashchange, move |_| {
        page.set(current_page());
        error.set(String::new());
        refresh();
    });

    let on_act = Callback::new(move |(command, run): Act| {
        spawn_local(async move {
            match tauri::invoke::<_, ()>(command, &Run { run }).await {
                Ok(()) => error.set(String::new()),
                Err(reason) => error.set(reason),
            }
            refresh();
        });
    });

    // The dialog on the Settings page that confirms no longer monitoring a
    // repository, or removing a host.
    let monitor_error = RwSignal::new(String::new());
    let host_error = RwSignal::new(String::new());
    let confirm_dialog = NodeRef::<Dialog>::new();
    let cancel = NodeRef::<Button>::new();
    let confirming = RwSignal::new(None::<Confirming>);
    let ask = move |question: Confirming| {
        confirming.set(Some(question));
        if let Some(dialog) = confirm_dialog.get_untracked() {
            dialog.set_return_value("");
            let _ = dialog.show_modal();
        }
        if let Some(cancel) = cancel.get_untracked() {
            let _ = cancel.focus();
        }
    };
    // Asks the app to `command` with `arguments`, showing why it couldn't.
    let act = move |command: &'static str, arguments: Destination| {
        spawn_local(async move {
            if let Err(reason) = tauri::invoke::<_, ()>(command, &arguments).await {
                error.set(reason);
            }
            refresh();
        });
    };
    let remove_host =
        move |destination: Option<String>| act("remove_host", Destination { destination });
    let stop_monitoring = move |repo: String| {
        spawn_local(async move {
            if let Err(reason) = tauri::invoke::<_, ()>("stop_monitoring", &Repo { repo }).await {
                error.set(reason);
            }
            refresh();
        });
    };
    let on_stop = Callback::new(move |repository: RepositoryRow| {
        if repository.missing {
            stop_monitoring(repository.repo);
            return;
        }
        ask(Confirming::Repository(repository));
    });
    let host_actions = HostActions {
        // A host with runs asks first, as a repository does.
        remove: Callback::new(move |host: HostRow| {
            if host.runs.is_empty() {
                remove_host(host.destination);
            } else {
                ask(Confirming::Host(host));
            }
        }),
        make_default: Callback::new(move |destination| {
            act("set_default_host", Destination { destination });
        }),
        check: Callback::new(move |destination| {
            act(
                "check_host",
                Destination {
                    destination: Some(destination),
                },
            );
        }),
    };
    let confirmed = move |_| {
        let ok = confirm_dialog
            .get_untracked()
            .is_some_and(|dialog| dialog.return_value() == "ok");
        match (ok, confirming.get_untracked()) {
            (true, Some(Confirming::Repository(repository))) => stop_monitoring(repository.repo),
            (true, Some(Confirming::Host(host))) => remove_host(host.destination),
            _ => {}
        }
    };
    let on_monitored = Callback::new(move |()| refresh());

    // The roles, read each time Settings opens so the form starts from
    // what is saved, and the agents whose setup isn't done.
    let agents = RwSignal::new(None::<Result<AgentSettings, String>>);
    let unready = RwSignal::new(Vec::<String>::new());
    Effect::new(move |_| {
        if page.get() != Page::Settings {
            return;
        }
        agents.set(None);
        spawn_local(async move {
            agents.set(Some(
                tauri::invoke::<_, AgentSettings>("agent_settings", &()).await,
            ));
            if let Ok(fetched) = tauri::invoke::<_, Vec<String>>("unready_agents", &()).await {
                unready.set(fetched);
            }
        });
    });

    // The dialog that uninstalls Epik. While it asks, it follows whether a
    // build is running.
    let uninstall_dialog = NodeRef::<Dialog>::new();
    let plan = RwSignal::new(Uninstall::default());
    let stage = RwSignal::new(Uninstalling::Asking(None));
    let asking = move || {
        matches!(stage.get_untracked(), Uninstalling::Asking(_))
            && uninstall_dialog
                .get_untracked()
                .is_some_and(|dialog| dialog.open())
    };
    set_interval(
        move || {
            if asking() {
                spawn_local(fetch_uninstall_plan(plan, stage));
            }
        },
        REFRESH,
    );
    let on_open_uninstall = Callback::new(move |()| {
        stage.set(Uninstalling::Asking(None));
        spawn_local(async move {
            fetch_uninstall_plan(plan, stage).await;
            if let Some(dialog) = uninstall_dialog.get_untracked() {
                let _ = dialog.show_modal();
            }
        });
    });
    let on_uninstall = Callback::new(move |()| {
        stage.set(Uninstalling::Removing);
        spawn_local(async move {
            match tauri::invoke::<_, Vec<UninstallStep>>("uninstall", &()).await {
                Ok(steps) => stage.set(Uninstalling::Done(steps)),
                Err(reason) => {
                    stage.set(Uninstalling::Asking(Some(reason)));
                    fetch_uninstall_plan(plan, stage).await;
                }
            }
        });
    });
    let on_cancel_uninstall = Callback::new(move |()| {
        if let Some(dialog) = uninstall_dialog.get_untracked() {
            dialog.close();
        }
    });
    let on_quit = Callback::new(move |()| tauri::run("quit_to_remove", ()));
    // Escape only closes the dialog while it asks.
    let escape = move |event: leptos::ev::Event| {
        if !matches!(stage.get_untracked(), Uninstalling::Asking(_)) {
            event.prevent_default();
        }
    };

    let which = Memo::new(move |_| {
        shown.with(|shown| match shown {
            None => Which::Nothing,
            Some(Shown::Home(_)) => Which::Home,
            Some(Shown::Run(page)) => Which::Run(page.run.clone()),
            Some(Shown::Settings(_)) => Which::Settings,
            Some(Shown::Failed(reason)) => Which::Failed(reason.clone()),
        })
    });
    let home = move || match shown.get() {
        Some(Shown::Home(home)) => Some(home),
        _ => None,
    };
    let run_page = move || match shown.get() {
        Some(Shown::Run(page)) => Some(*page),
        _ => None,
    };
    let settings = move || match shown.get() {
        Some(Shown::Settings(settings)) => Some(settings),
        _ => None,
    };
    let error_line = move || {
        let error = error.get();
        (!error.is_empty()).then(|| view! { <p class="error">{error}</p> })
    };
    let body = move || match which.get() {
        Which::Nothing => ().into_any(),
        Which::Failed(reason) => view! { <p class="error">{reason}</p> }.into_any(),
        Which::Home => {
            let home = untrack(home).map(|initial| kept(initial, home));
            view! {
                {error_line}
                {home.map(|home| view! { <HomePage home on_act/> })}
            }
            .into_any()
        }
        Which::Run(run) => {
            let page = untrack(run_page).map(|initial| kept(initial, run_page));
            let chosen = Signal::derive({
                let run = run.clone();
                move || {
                    let picked = selected.with(|selected| selected.get(&run).copied());
                    page?.with(|page| chosen_issue(page, picked))
                }
            });
            let picked = Signal::derive({
                let run = run.clone();
                move || picked_logs.with(|picked| picked.get(&run).cloned())
            });
            let on_choose = Callback::new({
                let run = run.clone();
                move |number: u64| {
                    selected.update(|selected| {
                        selected.insert(run.clone(), number);
                    });
                    // A new issue shows its own latest session.
                    picked_logs.update(|picked| {
                        picked.remove(&run);
                    });
                    refresh();
                }
            });
            let on_pick = Callback::new(move |log: String| {
                picked_logs.update(|picked| {
                    picked.insert(run.clone(), log);
                });
                refresh();
            });
            view! {
                {error_line}
                {page.map(|page| view! { <RunView page chosen on_choose streamed picked on_pick/> })}
            }
            .into_any()
        }
        Which::Settings => {
            let settings = untrack(settings).map(|initial| kept(initial, settings));
            view! {
                {error_line}
                {settings.map(|settings| view! {
                    <SettingsPage
                        settings
                        agents
                        unready
                        error=monitor_error
                        host_error
                        on_stop=Some(on_stop)
                        on_monitored=Some(on_monitored)
                        host_actions=Some(host_actions)
                        on_host_added=Some(on_monitored)
                        on_open_uninstall=Some(on_open_uninstall)
                    />
                })}
            }
            .into_any()
        }
    };

    view! {
        <nav id="sidebar"><Sidebar sidebar page/></nav>
        <main id="page">{body}</main>
        <dialog id="confirm" node_ref=confirm_dialog on:close=confirmed>
            {move || confirming.get().map(|question| match question {
                Confirming::Repository(repository) => view! { <ConfirmBody repository/> }.into_any(),
                Confirming::Host(host) => view! { <ConfirmHostBody host/> }.into_any(),
            })}
            <form method="dialog" class="buttons">
                <button value="cancel" id="confirm-cancel" autofocus node_ref=cancel>"Cancel"</button>
                <button value="ok" class="danger">
                    {move || match confirming.get() {
                        Some(Confirming::Host(_)) => "Remove",
                        _ => "Stop monitoring",
                    }}
                </button>
            </form>
        </dialog>
        <dialog id="uninstall" node_ref=uninstall_dialog on:cancel=escape>
            {move || {
                view! {
                    <UninstallBody
                        plan=plan.get()
                        stage=stage.get()
                        on_uninstall=Some(on_uninstall)
                        on_cancel=Some(on_cancel_uninstall)
                        on_quit=Some(on_quit)
                    />
                }
            }}
        </dialog>
    }
}
