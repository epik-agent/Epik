//! The tray menu: whether Epik is running, each active run with its
//! progress, and the way into Epik's window. It updates every second while
//! open, and its shortcuts work while it has focus.

use std::time::Duration;

use epik_view::{NodeState, Popover, TrayRun};
use leptos::ev::MouseEvent;
use leptos::prelude::*;
use serde::Serialize;
use wasm_bindgen::prelude::*;

use crate::tauri;

const REFRESH: Duration = Duration::from_secs(1);

/// What choosing something in the menu asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Open the main window at a route.
    OpenMain(String),
    /// Run a Tauri command that takes nothing.
    Run(&'static str),
}

#[derive(Serialize)]
struct OpenMain {
    route: String,
}

impl Command {
    /// Opens `run`'s page in the main window.
    pub fn open_run(run: &str) -> Command {
        Command::OpenMain(format!("run/{run}"))
    }

    fn send(self) {
        match self {
            Command::OpenMain(route) => tauri::run("open_main", OpenMain { route }),
            Command::Run(command) => tauri::run(command, ()),
        }
    }
}

/// An item of the menu, below the runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    OpenDashboard,
    Restart,
    Settings,
    CheckSetup,
    CheckForUpdates,
    Quit,
}

impl Item {
    /// Every item, top to bottom.
    pub const ALL: [Item; 6] = [
        Item::OpenDashboard,
        Item::Restart,
        Item::Settings,
        Item::CheckSetup,
        Item::CheckForUpdates,
        Item::Quit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Item::OpenDashboard => "Open Dashboard",
            Item::Restart => "Restart",
            Item::Settings => "Settings…",
            Item::CheckSetup => "Check setup…",
            Item::CheckForUpdates => "Check for updates…",
            Item::Quit => "Quit Epik",
        }
    }

    /// Its element's id.
    pub fn id(self) -> &'static str {
        match self {
            Item::OpenDashboard => "open-dashboard",
            Item::Restart => "restart",
            Item::Settings => "settings",
            Item::CheckSetup => "check-setup",
            Item::CheckForUpdates => "check-for-updates",
            Item::Quit => "quit",
        }
    }

    /// The key that, with Cmd (or Ctrl, off a Mac), chooses it.
    pub fn key(self) -> Option<char> {
        match self {
            Item::OpenDashboard => Some('D'),
            Item::Settings => Some(','),
            Item::Quit => Some('Q'),
            _ => None,
        }
    }

    pub fn command(self) -> Command {
        match self {
            Item::OpenDashboard => Command::OpenMain(String::new()),
            Item::Restart => Command::Run("restart"),
            Item::Settings => Command::OpenMain("settings".into()),
            Item::CheckSetup => Command::Run("check_setup"),
            Item::CheckForUpdates => Command::Run("check_for_updates"),
            Item::Quit => Command::Run("quit"),
        }
    }
}

/// The item a key pressed with Cmd or Ctrl chooses, if any.
pub fn shortcut(key: &str) -> Option<Item> {
    let mut chars = key.chars();
    let (Some(key), None) = (chars.next(), chars.next()) else {
        return None;
    };
    Item::ALL
        .into_iter()
        .find(|item| item.key() == Some(key.to_ascii_uppercase()))
}

/// That Epik is running, and its runs, agents and time up.
#[component]
pub fn Header(about: String) -> impl IntoView {
    view! {
        <header>
            <div class="running">
                <span class="dot"></span>
                <span class="name">"Epik is running"</span>
            </div>
            <div class="about">{about}</div>
        </header>
    }
}

/// A run's progress: a segment for each issue, in its state.
#[component]
pub fn Segments(segments: Vec<NodeState>) -> impl IntoView {
    let segments = segments
        .into_iter()
        .map(|state| {
            let class = match state {
                NodeState::Merged => "merged",
                NodeState::Running => "running",
                NodeState::NotStarted => "not_started",
                NodeState::Failed => "failed",
            };
            view! { <span class=class></span> }
        })
        .collect_view();
    view! { <div class="segments">{segments}</div> }
}

/// An active run, which opens its page.
#[component]
pub fn RunRow(run: TrayRun) -> impl IntoView {
    let command = Command::open_run(&run.run);
    view! {
        <button type="button" class="run" on:click=move |_| command.clone().send()>
            <div class="head">
                <span class="title">{run.title}</span>
                <span class="count">{run.count}</span>
            </div>
            <Segments segments=run.segments/>
            <div class="line">{run.line}</div>
        </button>
    }
}

/// The active runs under their heading; nothing when none is building.
#[component]
pub fn Runs(runs: Vec<TrayRun>) -> impl IntoView {
    (!runs.is_empty()).then(|| {
        let rows = runs
            .into_iter()
            .map(|run| view! { <RunRow run/> })
            .collect_view();
        view! {
            <section class="runs">
                <h2>"Active runs"</h2>
                {rows}
            </section>
        }
    })
}

/// An item, with its shortcut if it has one.
#[component]
fn ItemButton(item: Item, mac: bool) -> impl IntoView {
    let shortcut = item.key().map(|key| {
        let shown = if mac {
            format!("⌘{key}")
        } else {
            format!("Ctrl+{key}")
        };
        view! { <span class="shortcut">{shown}</span> }
    });
    let choose = move |event: MouseEvent| {
        event.prevent_default();
        item.command().send();
    };
    view! {
        <button type="button" class="item" id=item.id() on:click=choose>
            <span class="label">{item.label()}</span>
            {shortcut}
        </button>
    }
}

/// The items, grouped: the dashboard, Restart, then the rest. Shortcuts
/// are drawn with Cmd on a Mac and Ctrl elsewhere.
#[component]
pub fn Items(mac: bool) -> impl IntoView {
    let button = move |item| view! { <ItemButton item mac/> };
    let rest = Item::ALL[2..]
        .iter()
        .map(|&item| button(item))
        .collect_view();
    view! {
        <nav class="items">
            {button(Item::OpenDashboard)}
            <hr/>
            {button(Item::Restart)}
            <hr/>
            {rest}
        </nav>
    }
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(thread_local_v2, js_namespace = navigator, js_name = userAgent)]
    static USER_AGENT: String;
}

/// Whether the window is on a Mac, as its user agent says.
fn on_a_mac() -> bool {
    USER_AGENT.with(|agent| agent.contains("Mac"))
}

/// The menu's window, refreshed from the app every second.
#[component]
pub fn PopoverWindow() -> impl IntoView {
    let drawn = RwSignal::new(None::<Result<Popover, String>>);
    let refresh = move || {
        leptos::task::spawn_local(async move {
            drawn.set(Some(tauri::invoke::<_, Popover>("popover", &()).await));
        });
    };
    refresh();
    set_interval(refresh, REFRESH);
    let keys = window_event_listener(leptos::ev::keydown, |event| {
        if !(event.meta_key() || event.ctrl_key()) {
            return;
        }
        if let Some(item) = shortcut(&event.key()) {
            event.prevent_default();
            item.command().send();
        }
    });
    on_cleanup(move || keys.remove());
    let mac = on_a_mac();
    view! {
        {move || match drawn.get() {
            Some(Ok(popover)) => view! {
                <Header about=popover.about/>
                <Runs runs=popover.runs/>
            }
            .into_any(),
            Some(Err(reason)) => view! { <p class="error">{reason}</p> }.into_any(),
            None => ().into_any(),
        }}
        <Items mac/>
    }
}
