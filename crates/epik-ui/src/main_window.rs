//! The main window: Home (`#/`), a run's page (`#/run/<owner/name%23n>`),
//! and Settings over Home. It asks the app what to show every second and
//! draws it. Its only controls are Rerun and Delete on a run that needs
//! attention, and Settings.

use std::collections::HashMap;
use std::time::Duration;

use epik_view::{Home, NodeState, RepositoryRow, RunPage, Settings};
use leptos::ev::MouseEvent;
use leptos::html::{Button, Dialog};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;

use crate::flash::{Flashes, Flashing};
use crate::home::{Act, HomePage};
use crate::live::kept;
use crate::run_page::RunView;
use crate::settings::{ConfirmBody, MonitorForm, Repositories};
use crate::tauri;

const REFRESH: Duration = Duration::from_secs(1);

/// A page of the main window, as its URL's hash names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Page {
    /// `#/`, and any hash that isn't a run's page.
    Home,
    /// `#/run/<owner/name%23n>`: the run's page.
    Run(String),
}

impl Page {
    pub fn from_hash(hash: &str) -> Page {
        let path = hash.strip_prefix('#').unwrap_or(hash);
        let path = decode(path.strip_prefix('/').unwrap_or(path));
        match path.strip_prefix("run/") {
            Some(run) => Page::Run(run.to_owned()),
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
/// page. A run's `#` would end the URL's fragment, so it is escaped.
pub fn href(path: &str) -> String {
    format!("#/{}", path.replace('#', "%23"))
}

/// Epik's mark, set as the span's HTML: Leptos would wrap the static SVG's
/// children in groups.
const MARK: &str = r##"<svg viewBox="0 0 64 64" width="20" height="20" aria-hidden="true"><g stroke="#171717" stroke-width="3" opacity="0.3"><line x1="16" y1="16" x2="48" y2="24"/><line x1="16" y1="16" x2="24" y2="48"/><line x1="48" y1="24" x2="40" y2="48"/><line x1="24" y1="48" x2="40" y2="48"/></g><circle cx="16" cy="16" r="7" fill="#00c07a"/><circle cx="48" cy="24" r="6" fill="#171717"/><circle cx="24" cy="48" r="6" fill="#171717"/><circle cx="40" cy="48" r="5" fill="#00c07a" opacity="0.7"/></svg>"##;

/// The bar on Home: the mark, the name, the headline, and the Settings
/// gear, marked when a monitored repository is no longer on GitHub.
#[component]
pub fn HomeBar(
    headline: String,
    missing_repository: bool,
    #[prop(optional)] on_settings: Option<Callback<()>>,
) -> impl IntoView {
    let settings = move |_: MouseEvent| {
        if let Some(on_settings) = on_settings {
            on_settings.run(());
        }
    };
    let badge = missing_repository.then(|| view! { <span class="badge">"✕"</span> });
    view! {
        <span class="mark" inner_html=MARK></span>
        <span class="name">"Epik"</span>
        <span class="headline">{headline}</span>
        <button class="gear" title="Settings" aria-label="Settings" on:click=settings>
            "⚙"
            {badge}
        </button>
    }
}

/// The bar on a run's page: the run, a link back to Home, and links to the
/// other runs building.
#[component]
pub fn RunBar(title: String, also_building: Vec<(String, String)>) -> impl IntoView {
    let also = (!also_building.is_empty()).then(|| {
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
    });
    view! {
        <span class="mark" inner_html=MARK></span>
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
    Failed(String),
}

/// Which page the window draws. The page's nodes stay while it doesn't
/// change.
#[derive(Debug, Clone, PartialEq)]
enum Which {
    Nothing,
    Home,
    Run(String),
    Failed(String),
}

/// What the bar shows: it stays when a refresh fails.
#[derive(Debug, Clone, PartialEq)]
enum Bar {
    Nothing,
    Home(String, bool),
    Run(String, Vec<(String, String)>),
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
        Shown::Failed(_) => vec![],
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
struct Log {
    path: String,
}

fn current_page() -> Page {
    Page::from_hash(&window().location().hash().unwrap_or_default())
}

/// Fetches Settings again.
async fn fetch_settings(settings: RwSignal<Option<Result<Settings, String>>>) {
    settings.set(Some(tauri::invoke::<_, Settings>("settings", &()).await));
}

/// The main window, refreshed from the app every second.
#[component]
pub fn MainWindow() -> impl IntoView {
    let page = RwSignal::new(current_page());
    let shown = RwSignal::new(None::<Shown>);
    // What the user asked for that failed, until the page changes.
    let error = RwSignal::new(String::new());
    // The issue chosen on each run's page.
    let selected = RwSignal::new(HashMap::<String, u64>::new());
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
            let at = page.get_untracked();
            let fetched = match &at {
                Page::Home => tauri::invoke::<_, Home>("home", &()).await.map(Shown::Home),
                Page::Run(run) => {
                    tauri::invoke::<_, RunPage>("run_page", &Run { run: run.clone() })
                        .await
                        .map(|page| Shown::Run(Box::new(page)))
                }
            };
            match fetched {
                Ok(fetched) => {
                    let title = match &fetched {
                        Shown::Run(page) => format!("{} · Epik", page.title),
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
    let on_open_log = Callback::new(move |path: String| {
        spawn_local(async move {
            if let Err(reason) = tauri::invoke::<_, ()>("open_log", &Log { path }).await {
                error.set(reason);
            }
        });
    });

    // Settings, and the dialog that confirms no longer monitoring a
    // repository.
    let settings = RwSignal::new(None::<Result<Settings, String>>);
    let monitor_error = RwSignal::new(String::new());
    let settings_dialog = NodeRef::<Dialog>::new();
    let confirm_dialog = NodeRef::<Dialog>::new();
    let cancel = NodeRef::<Button>::new();
    let confirming = RwSignal::new(None::<RepositoryRow>);
    let on_settings = Callback::new(move |()| {
        spawn_local(async move {
            fetch_settings(settings).await;
            if let Some(dialog) = settings_dialog.get_untracked() {
                let _ = dialog.show_modal();
            }
        });
    });
    let stop_monitoring = move |repo: String| {
        spawn_local(async move {
            if let Err(reason) = tauri::invoke::<_, ()>("stop_monitoring", &Repo { repo }).await {
                error.set(reason);
            }
            fetch_settings(settings).await;
            refresh();
        });
    };
    let on_stop = Callback::new(move |repository: RepositoryRow| {
        if repository.missing {
            stop_monitoring(repository.repo);
            return;
        }
        confirming.set(Some(repository));
        if let Some(dialog) = confirm_dialog.get_untracked() {
            dialog.set_return_value("");
            let _ = dialog.show_modal();
        }
        if let Some(cancel) = cancel.get_untracked() {
            let _ = cancel.focus();
        }
    });
    let confirmed = move |_| {
        let ok = confirm_dialog
            .get_untracked()
            .is_some_and(|dialog| dialog.return_value() == "ok");
        if let (true, Some(repository)) = (ok, confirming.get_untracked()) {
            stop_monitoring(repository.repo);
        }
    };
    let on_monitored = Callback::new(move |()| spawn_local(fetch_settings(settings)));

    let bar = Memo::new(move |before: Option<&Bar>| {
        shown.with(|shown| match shown {
            Some(Shown::Home(home)) => Bar::Home(home.headline.clone(), home.missing_repository),
            Some(Shown::Run(page)) => Bar::Run(page.title.clone(), page.also_building.clone()),
            _ => before.cloned().unwrap_or(Bar::Nothing),
        })
    });
    let bar = move || match bar.get() {
        Bar::Nothing => ().into_any(),
        Bar::Home(headline, missing_repository) => {
            view! { <HomeBar headline missing_repository on_settings/> }.into_any()
        }
        Bar::Run(title, also_building) => view! { <RunBar title also_building/> }.into_any(),
    };

    let which = Memo::new(move |_| {
        shown.with(|shown| match shown {
            None => Which::Nothing,
            Some(Shown::Home(_)) => Which::Home,
            Some(Shown::Run(page)) => Which::Run(page.run.clone()),
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
                    picked.or_else(|| {
                        let page = page?;
                        page.with(|page| {
                            page.issues
                                .iter()
                                .find(|issue| issue.state == NodeState::Running)
                                .or(page.issues.first())
                                .map(|issue| issue.number)
                        })
                    })
                }
            });
            let on_choose = Callback::new(move |number: u64| {
                selected.update(|selected| {
                    selected.insert(run.clone(), number);
                });
            });
            view! {
                {error_line}
                {page.map(|page| view! { <RunView page chosen on_choose on_open_log/> })}
            }
            .into_any()
        }
    };

    view! {
        <header id="bar">{bar}</header>
        <main id="page">{body}</main>
        <dialog id="settings" node_ref=settings_dialog>
            <form method="dialog" class="dialog-head">
                <h2>"Settings"</h2>
                <button class="close" aria-label="Close">"✕"</button>
            </form>
            <section>
                <h3>"Monitored repositories"</h3>
                <p class="muted">
                    "Epik monitors these repositories. When Epik creates an epic or starts a build in a repository that isn't listed, it adds that repository here."
                </p>
                <ul id="repositories">
                    {move || settings.get().map(|settings| view! { <Repositories settings on_stop=Some(on_stop)/> })}
                </ul>
                <MonitorForm error=monitor_error on_monitored=Some(on_monitored)/>
            </section>
        </dialog>
        <dialog id="confirm" node_ref=confirm_dialog on:close=confirmed>
            {move || confirming.get().map(|repository| view! { <ConfirmBody repository/> })}
            <form method="dialog" class="buttons">
                <button value="cancel" id="confirm-cancel" autofocus node_ref=cancel>"Cancel"</button>
                <button value="ok" class="danger">"Stop monitoring"</button>
            </form>
        </dialog>
    }
}
