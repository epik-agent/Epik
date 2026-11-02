//! Settings, a page of the main window: the agent and model each role runs
//! with, the monitored repositories, a form to monitor another, the hosts builds can run on, a form to add another,
//! and a button to uninstall Epik.

use std::path::Path;

use epik_view::{
    AgentSettings, HostRow, HostStatus, Removal, RepositoryRow, Role, Settings, Uninstall,
    UninstallStep,
};
use leptos::ev::SubmitEvent;
use leptos::html::Input;
use leptos::prelude::*;
use serde::Serialize;

use crate::tauri;
use crate::words::GithubLink;

/// Where the docs say how to set up a host.
const HOST_DOCS: &str = "https://github.com/epik-agent/Epik#build-on-another-machine";

/// What a host's buttons ask for.
#[derive(Clone, Copy)]
pub struct HostActions {
    /// Remove this host.
    pub remove: Callback<HostRow>,
    /// Make this host, or this computer for none, the default for builds.
    pub make_default: Callback<Option<String>>,
    /// Check this host again.
    pub check: Callback<String>,
}

/// The Settings page: the form choosing each role's agent and model, once
/// they are read, the monitored repositories, each with its button,
/// the form to monitor another, the hosts, each with its buttons, the form
/// to add another, and the button that opens the confirmation to uninstall
/// Epik. The lists are redrawn only when `settings` changes, so the forms
/// keep what is typed in them.
#[component]
pub fn SettingsPage(
    #[prop(into)] settings: Signal<Result<Settings, String>>,
    /// The roles, or why they couldn't be read, or `None` until they are.
    #[prop(into, optional)]
    agents: Signal<Option<Result<AgentSettings, String>>>,
    /// The ids of the agents whose setup isn't done.
    #[prop(into, optional)]
    unready: Signal<Vec<String>>,
    error: RwSignal<String>,
    host_error: RwSignal<String>,
    #[prop(optional_no_strip)] on_stop: Option<Callback<RepositoryRow>>,
    #[prop(optional_no_strip)] on_monitored: Option<Callback<()>>,
    #[prop(optional_no_strip)] host_actions: Option<HostActions>,
    #[prop(optional_no_strip)] on_host_added: Option<Callback<()>>,
    #[prop(optional_no_strip)] on_open_uninstall: Option<Callback<()>>,
) -> impl IntoView {
    let settings = Memo::new(move |_| settings.get());
    let agents = Memo::new(move |_| agents.get());
    let roles = move || {
        agents.get().map(|agents| match agents {
            Ok(agents) => view! { <Agents agents unready/> }.into_any(),
            Err(reason) => view! { <p class="error">{reason}</p> }.into_any(),
        })
    };
    let hosts = Memo::new(move |_| {
        settings.with(|settings| {
            settings
                .as_ref()
                .map(|settings| settings.hosts.clone())
                .unwrap_or_default()
        })
    });
    view! {
        <h1 class="page-title">"Settings"</h1>
        <div class="card settings">
            <section class="agents">
                <h3>"Agents"</h3>
                {roles}
            </section>
            <section>
                <h3>"Monitored repositories"</h3>
                <p class="muted">
                    "Epik monitors these repositories. When Epik creates an epic or starts a build in a repository that isn't listed, it adds that repository here."
                </p>
                <ul id="repositories">
                    {move || view! { <Repositories settings=settings.get() on_stop/> }}
                </ul>
                <MonitorForm error on_monitored/>
            </section>
            <section class="hosts">
                <h3>"Hosts"</h3>
                <p class="muted">
                    "Builds run on one of these machines: this one, or another you reach with ssh that runs Epik. "
                    <GithubLink text="How to set up a host".to_owned() url=HOST_DOCS.to_owned()/>
                </p>
                <ul id="hosts">
                    {move || view! { <Hosts hosts=hosts.get() actions=host_actions/> }}
                </ul>
                <HostForm error=host_error on_added=on_host_added/>
            </section>
            <UninstallButton on_open=on_open_uninstall/>
        </div>
    }
}

/// The agents a role can run: each one's id, name, and how it runs
/// unattended.
const AGENTS: [(&str, &str, &str); 2] = [
    (
        "claude",
        "Claude Code",
        "runs unattended with a model approving each risky action, and no sandbox.",
    ),
    (
        "codex",
        "Codex",
        "runs unattended in a sandbox, with its own reviewer approving what leaves it.",
    ),
];

/// What review's agent field says when nothing reviews.
const OFF: &str = "off";

/// What the form choosing the roles holds: each role's agent id, or
/// [`OFF`] for no review, and the model as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub coding: String,
    pub coding_model: String,
    pub review: String,
    pub review_model: String,
}

impl From<&AgentSettings> for Choice {
    fn from(settings: &AgentSettings) -> Self {
        let model =
            |role: Option<&Role>| role.and_then(|role| role.model.clone()).unwrap_or_default();
        Self {
            coding: settings.coding.agent.clone(),
            coding_model: model(Some(&settings.coding)),
            review: settings
                .review
                .as_ref()
                .map_or(OFF.to_owned(), |role| role.agent.clone()),
            review_model: model(settings.review.as_ref()),
        }
    }
}

impl Choice {
    /// The roles as chosen. A blank model is the agent's default.
    pub fn settings(&self) -> AgentSettings {
        let role = |agent: &str, model: &str| Role {
            agent: agent.to_owned(),
            model: Some(model.trim())
                .filter(|model| !model.is_empty())
                .map(str::to_owned),
        };
        AgentSettings {
            coding: role(&self.coding, &self.coding_model),
            review: (self.review != OFF).then(|| role(&self.review, &self.review_model)),
        }
    }

    /// What saving the roles as chosen asks of the app.
    pub fn request(&self) -> SetAgentSettings {
        SetAgentSettings {
            settings: self.settings(),
        }
    }

    /// Whether a role runs the agent `id`.
    fn uses(&self, id: &str) -> bool {
        self.coding == id || self.review == id
    }
}

/// The app's command that keeps the roles, with its argument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SetAgentSettings {
    pub settings: AgentSettings,
}

impl SetAgentSettings {
    pub const COMMAND: &str = "set_agent_settings";
}

/// The form choosing each role's agent and model, for the builds that
/// start after it is saved, and each agent in a line saying how it runs
/// unattended. An agent a role uses whose setup isn't done says so, and
/// opens the setup checklist.
#[component]
pub fn Agents(
    agents: AgentSettings,
    /// The ids of the agents whose setup isn't done.
    #[prop(into, optional)]
    unready: Signal<Vec<String>>,
) -> impl IntoView {
    let choice = RwSignal::new(Choice::from(&agents));
    let error = RwSignal::new(String::new());
    let saved = RwSignal::new(false);
    let saving = RwSignal::new(false);
    let change = move |set: fn(&mut Choice, String)| {
        move |event: leptos::ev::Event| {
            let value = event_target_value(&event);
            choice.update(|choice| set(choice, value));
            saved.set(false);
        }
    };
    let options = move |field: fn(&Choice) -> &str, off: bool| {
        let off = off.then(|| {
            view! {
                <option value=OFF selected=move || choice.with(|choice| field(choice) == OFF)>
                    "Off"
                </option>
            }
        });
        let agents = AGENTS
            .into_iter()
            .map(|(id, name, _)| {
                view! {
                    <option value=id selected=move || choice.with(|choice| field(choice) == id)>
                        {name}
                    </option>
                }
            })
            .collect_view();
        view! { {off}{agents} }
    };
    let model =
        move |id: &'static str, field: fn(&Choice) -> &str, set: fn(&mut Choice, String)| {
            view! {
                <input
                    id=id
                    placeholder="The agent's default"
                    autocomplete="off"
                    spellcheck="false"
                    value=choice.with_untracked(|choice| field(choice).to_owned())
                    on:input=change(set)
                />
            }
        };
    let review_model = move || {
        choice.with(|choice| choice.review != OFF).then(|| {
            model(
                "review-model",
                |choice| &choice.review_model,
                |choice, value| choice.review_model = value,
            )
        })
    };
    let lines = AGENTS
        .into_iter()
        .map(|(id, name, unattended)| {
            let setup = move || {
                let unready = choice.with(|choice| choice.uses(id))
                    && unready.with(|unready| unready.iter().any(|agent| agent == id));
                unready.then(|| {
                    view! {
                        " "
                        <span class="why">"Its setup isn't done. "</span>
                        <button type="button" class="link" on:click=|_| tauri::run("check_setup", ())>
                            "Check setup"
                        </button>
                    }
                })
            };
            view! { <li><b>{name}</b>{format!(" {unattended}")}{setup}</li> }
        })
        .collect_view();
    let submit = move |event: SubmitEvent| {
        event.prevent_default();
        saving.set(true);
        let request = choice.get_untracked().request();
        leptos::task::spawn_local(async move {
            match tauri::invoke::<_, ()>(SetAgentSettings::COMMAND, &request).await {
                Ok(()) => {
                    error.set(String::new());
                    saved.set(true);
                }
                Err(reason) => error.set(reason),
            }
            saving.set(false);
        });
    };
    view! {
        <form id="agents" on:submit=submit>
            <div class="role">
                <label for="coding-agent">"Coding"</label>
                <select id="coding-agent" on:change=change(|choice, value| choice.coding = value)>
                    {options(|choice| &choice.coding, false)}
                </select>
                {model(
                    "coding-model",
                    |choice| &choice.coding_model,
                    |choice, value| choice.coding_model = value,
                )}
            </div>
            <div class="role">
                <label for="review-agent">"Review"</label>
                <select id="review-agent" on:change=change(|choice, value| choice.review = value)>
                    {options(|choice| &choice.review, true)}
                </select>
                {review_model}
            </div>
            <p class="muted">
                "Reviews the build's pull request when the build is done, and fixes what it finds, up to twice, before the pull request reaches you."
            </p>
            <ul class="agent-list">{lines}</ul>
            <div class="row">
                <button class="primary" disabled=move || saving.get()>"Save"</button>
                <span class="muted">
                    {move || saved.get().then_some("Saved. Builds that start from now on use these.")}
                </span>
            </div>
            <p id="agents-error" class="error">{error}</p>
        </form>
    }
}

/// The monitored repositories' rows, each with its button, or why they
/// couldn't be read. A missing repository's button removes it; another's
/// asks first.
#[component]
pub fn Repositories(
    settings: Result<Settings, String>,
    #[prop(optional_no_strip)] on_stop: Option<Callback<RepositoryRow>>,
) -> impl IntoView {
    let settings = match settings {
        Ok(settings) => settings,
        Err(reason) => return view! { <li class="error">{reason}</li> }.into_any(),
    };
    if settings.repositories.is_empty() {
        return view! { <li class="muted none">"No repositories yet."</li> }.into_any();
    }
    settings
        .repositories
        .into_iter()
        .map(|repository| {
            let missing = repository.missing;
            let (text, url) = (repository.repo.clone(), repository.url.clone());
            let about = if missing {
                view! { <div class="why">"No longer on GitHub. It was deleted, renamed, or you lost access."</div> }
                    .into_any()
            } else {
                view! { <div class="muted">{repository.about.clone()}</div> }.into_any()
            };
            let stop = move |_| {
                if let Some(on_stop) = on_stop {
                    on_stop.run(repository.clone());
                }
            };
            view! {
                <li class=if missing { "repository missing" } else { "repository" }>
                    <span class=if missing { "status-x" } else { "status-dot" }>
                        {missing.then_some("✕")}
                    </span>
                    <div class="repo-text">
                        <GithubLink text url/>
                        {about}
                    </div>
                    <button on:click=stop>{if missing { "Remove" } else { "Stop monitoring" }}</button>
                </li>
            }
        })
        .collect_view()
        .into_any()
}

/// What the dialog that confirms no longer monitoring `repository` asks,
/// with the runs that would leave the window.
#[component]
pub fn ConfirmBody(repository: RepositoryRow) -> impl IntoView {
    let text = match repository.runs.len() {
        0 => "Epik will stop checking it. It has no runs in the window.".to_owned(),
        1 => "Epik will stop checking it, and this run will disappear from the window:".to_owned(),
        n => format!(
            "Epik will stop checking it, and these {n} runs will disappear from the window:"
        ),
    };
    let runs = repository
        .runs
        .into_iter()
        .map(|(run, title)| view! { <li>{format!("{title} ({run})")}</li> })
        .collect_view();
    view! {
        <h2 id="confirm-title">{format!("Stop monitoring {}?", repository.repo)}</h2>
        <p id="confirm-text">{text}</p>
        <ul id="confirm-runs">{runs}</ul>
    }
}

#[derive(Serialize)]
struct Monitor {
    repo: String,
}

/// The form to monitor another repository, with why the app wouldn't
/// under its input. Once the app has answered, it calls `on_monitored`.
#[component]
pub fn MonitorForm(
    error: RwSignal<String>,
    #[prop(optional_no_strip)] on_monitored: Option<Callback<()>>,
) -> impl IntoView {
    let input = NodeRef::<Input>::new();
    let submit = move |event: SubmitEvent| {
        event.prevent_default();
        let Some(input) = input.get() else {
            return;
        };
        leptos::task::spawn_local(async move {
            let repo = Monitor {
                repo: input.value(),
            };
            match tauri::invoke::<_, ()>("monitor", &repo).await {
                Ok(()) => {
                    input.set_value("");
                    error.set(String::new());
                }
                Err(reason) => error.set(reason),
            }
            if let Some(on_monitored) = on_monitored {
                on_monitored.run(());
            }
        });
    };
    view! {
        <form id="add" on:submit=submit>
            <label for="repo">"Monitor another repository"</label>
            <div class="row">
                <input id="repo" placeholder="owner/name" autocomplete="off" spellcheck="false" node_ref=input/>
                <button class="primary">"Add"</button>
            </div>
            <p id="add-error" class="error">{error}</p>
        </form>
    }
}

/// The hosts' rows: each with its mark, what was last heard from it, why
/// it can't build and what it lacks, and its buttons. This computer can
/// only be made the default; a remote host can also be checked again and
/// removed.
#[component]
pub fn Hosts(
    hosts: Vec<HostRow>,
    #[prop(optional_no_strip)] actions: Option<HostActions>,
) -> impl IntoView {
    hosts
        .into_iter()
        .map(|host| {
            let (class, mark) = match host.status {
                HostStatus::Ready => (
                    "host",
                    view! { <span class="status-dot"></span> }.into_any(),
                ),
                HostStatus::Unchecked => (
                    "host unchecked",
                    view! { <span class="status-none"></span> }.into_any(),
                ),
                HostStatus::Incomplete => (
                    "host incomplete",
                    view! { <span class="status-warn">"!"</span> }.into_any(),
                ),
                HostStatus::Unreachable | HostStatus::Mismatch => (
                    "host unreachable",
                    view! { <span class="status-x">"✕"</span> }.into_any(),
                ),
            };
            let why = host
                .why
                .clone()
                .map(|why| view! { <div class="why">{why}</div> });
            let fixes = (!host.missing.is_empty()).then(|| {
                let name = host.name.clone();
                let fixes = host
                    .missing
                    .iter()
                    .map(|fix| {
                        let commands = fix
                            .commands
                            .iter()
                            .map(|command| view! { <code>{command.clone()}</code> })
                            .collect_view();
                        view! {
                            <li>
                                {format!("{}: {}. On {name}, run:", fix.title, fix.detail)}
                                <pre>{commands}</pre>
                            </li>
                        }
                    })
                    .collect_view();
                view! { <ul class="fixes">{fixes}</ul> }
            });
            let destination = host.destination.clone();
            let make_default = {
                let destination = destination.clone();
                move |_| {
                    if let Some(actions) = actions {
                        actions.make_default.run(destination.clone());
                    }
                }
            };
            let default = if host.default {
                view! { <span class="default">"Default for builds"</span> }.into_any()
            } else {
                view! { <button on:click=make_default>"Make default"</button> }.into_any()
            };
            let remote = destination.map(|destination| {
                let check = move |_| {
                    if let Some(actions) = actions {
                        actions.check.run(destination.clone());
                    }
                };
                let row = host.clone();
                let remove = move |_| {
                    if let Some(actions) = actions {
                        actions.remove.run(row.clone());
                    }
                };
                view! {
                    <button on:click=check>"Check again"</button>
                    <button on:click=remove>"Remove"</button>
                }
            });
            view! {
                <li class=class>
                    {mark}
                    <div class="host-text">
                        <div class="host-name">{host.name.clone()}</div>
                        <div class="muted">{host.about.clone()}</div>
                        {why}
                        {fixes}
                    </div>
                    <div class="host-buttons">{default}{remote}</div>
                </li>
            }
        })
        .collect_view()
}

/// What the dialog that confirms removing `host` asks, with the runs that
/// would leave the window.
#[component]
pub fn ConfirmHostBody(host: HostRow) -> impl IntoView {
    let text = match host.runs.len() {
        0 => "Epik will stop checking it. It has no runs in the window.".to_owned(),
        1 => format!(
            "Epik will stop checking it, and this run will disappear from the window. The run stays on {}.",
            host.name
        ),
        n => format!(
            "Epik will stop checking it, and these {n} runs will disappear from the window. They stay on {}.",
            host.name
        ),
    };
    let runs = host
        .runs
        .into_iter()
        .map(|(run, title)| view! { <li>{format!("{title} ({run})")}</li> })
        .collect_view();
    view! {
        <h2 id="confirm-title">{format!("Remove {}?", host.name)}</h2>
        <p id="confirm-text">{text}</p>
        <ul id="confirm-runs">{runs}</ul>
    }
}

#[derive(Serialize)]
struct AddHost {
    destination: String,
}

/// The form to add a host by its SSH destination, with why the app
/// wouldn't under its input. Adding checks the host, which can take a
/// while; once the app has answered, it calls `on_added`.
#[component]
pub fn HostForm(
    error: RwSignal<String>,
    #[prop(optional_no_strip)] on_added: Option<Callback<()>>,
) -> impl IntoView {
    let input = NodeRef::<Input>::new();
    let adding = RwSignal::new(false);
    let submit = move |event: SubmitEvent| {
        event.prevent_default();
        let Some(input) = input.get() else {
            return;
        };
        adding.set(true);
        leptos::task::spawn_local(async move {
            let host = AddHost {
                destination: input.value(),
            };
            match tauri::invoke::<_, ()>("add_host", &host).await {
                Ok(()) => {
                    input.set_value("");
                    error.set(String::new());
                }
                Err(reason) => error.set(reason),
            }
            adding.set(false);
            if let Some(on_added) = on_added {
                on_added.run(());
            }
        });
    };
    view! {
        <form id="add-host" on:submit=submit>
            <label for="host">"Add a host"</label>
            <div class="row">
                <input id="host" placeholder="devbox or me@build.example.com" autocomplete="off" spellcheck="false" node_ref=input/>
                <button class="primary" disabled=move || adding.get()>
                    {move || if adding.get() { "Checking…" } else { "Add" }}
                </button>
            </div>
            <p id="add-host-error" class="error">{error}</p>
        </form>
    }
}

/// The last part of Settings, set apart from the repositories: the button
/// that opens the confirmation to uninstall Epik.
#[component]
pub fn UninstallButton(#[prop(optional_no_strip)] on_open: Option<Callback<()>>) -> impl IntoView {
    let open = move |_| {
        if let Some(on_open) = on_open {
            on_open.run(());
        }
    };
    view! {
        <section class="uninstall">
            <h3>"Uninstall"</h3>
            <button class="danger-outline" on:click=open>"Uninstall Epik…"</button>
        </section>
    }
}

/// Where uninstalling stands, as its dialog shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Uninstalling {
    /// Asking to confirm, with why the app refused, if it did.
    Asking(Option<String>),
    /// Removing Epik's files.
    Removing,
    /// Finished, with how each step went.
    Done(Vec<UninstallStep>),
}

/// How to remove the app the way the platform removes any app, the second
/// of uninstalling's two steps, as the end of a sentence.
fn removal_line(removal: &Removal) -> AnyView {
    match removal {
        Removal::MacApp(bundle) => {
            let folder = match bundle.parent() {
                Some(folder) if folder == Path::new("/Applications") => "Applications".to_owned(),
                Some(folder) => folder.display().to_string(),
                None => "its folder".to_owned(),
            };
            view! { {format!("drag Epik from {folder} to the Trash.")} }.into_any()
        }
        Removal::WindowsApp => {
            view! { "open Settings › Apps › Installed apps, and uninstall Epik there." }.into_any()
        }
        Removal::AppImage(_) => view! { "delete the AppImage." }.into_any(),
        Removal::Deb => view! { "run "<code>"sudo apt remove epik"</code>"." }.into_any(),
    }
}

/// What the Done button says: it does the platform's step for the user
/// where it can, then quits.
fn quit_label(removal: &Removal) -> &'static str {
    match removal {
        Removal::MacApp(_) => "Quit and Show in Finder",
        Removal::WindowsApp => "Quit and Open Installed Apps",
        Removal::AppImage(_) => "Quit and Show the AppImage",
        Removal::Deb => "Quit",
    }
}

/// The dialog that uninstalls Epik, in two steps: Epik removes its own
/// files, then the user quits and removes the app the way their platform
/// removes any app. It says what goes, what stays, and how to remove the
/// app, then how each step went, then offers to quit, doing the platform's
/// step first where it can. Uninstall is disabled while a build runs.
#[component]
pub fn UninstallBody(
    plan: Uninstall,
    stage: Uninstalling,
    #[prop(optional_no_strip)] on_uninstall: Option<Callback<()>>,
    #[prop(optional_no_strip)] on_cancel: Option<Callback<()>>,
    #[prop(optional_no_strip)] on_quit: Option<Callback<()>>,
) -> impl IntoView {
    let call = |callback: Option<Callback<()>>| {
        move |_| {
            if let Some(callback) = callback {
                callback.run(());
            }
        }
    };
    match stage {
        Uninstalling::Asking(error) => {
            let history = (plan.runs > 0).then(|| {
                let runs = if plan.runs == 1 {
                    "1 run".to_owned()
                } else {
                    format!("{} runs", plan.runs)
                };
                view! {
                    <p class="warning">
                        {format!("The history of {runs} will be deleted and can't be recovered.")}
                    </p>
                }
            });
            let building = plan.building.then(|| {
                view! { <p class="why">"Wait for the running build to finish, or stop it first."</p> }
            });
            let error = error.map(|error| view! { <p class="error">{error}</p> });
            view! {
                <h2>"Uninstall Epik?"</h2>
                <p>"Uninstalling removes:"</p>
                <ul class="uninstall-list">
                    <li>"The Epik plugin and its marketplace, from Claude Code"</li>
                    <li>"Epik's own copies of gh and Node"</li>
                    <li>"Epik's link to the app, and its temporary build clones"</li>
                    <li>"Everything else in ~/.epik, including the run history"</li>
                </ul>
                <p>"It leaves alone what you may also use without Epik:"</p>
                <ul class="uninstall-list">
                    <li>"Claude Code, and your sign-in to it"</li>
                    <li>"gh on your PATH, your sign-in to GitHub, and git pushing with gh"</li>
                </ul>
                <p>
                    "Uninstalling has two steps. First Epik removes its own files, listed above. \
                     Then you quit Epik and remove the app the way you'd remove any app: "
                    {removal_line(&plan.removal)}
                </p>
                {history}
                {building}
                {error}
                <div class="buttons">
                    <button on:click=call(on_cancel)>"Cancel"</button>
                    <button class="danger" disabled=plan.building on:click=call(on_uninstall)>
                        "Uninstall"
                    </button>
                </div>
            }
            .into_any()
        }
        Uninstalling::Removing => view! {
            <h2>"Uninstalling Epik…"</h2>
            <p class="muted">"Removing Epik's files."</p>
        }
        .into_any(),
        Uninstalling::Done(steps) => {
            let failed = steps.iter().any(|step| step.error.is_some());
            let (title, text) = if failed {
                (
                    "Epik is partly uninstalled",
                    "Epik couldn't remove everything. Remove what failed yourself, then quit Epik and ",
                )
            } else {
                (
                    "Epik's files are removed",
                    "One step left: quit Epik, then ",
                )
            };
            let steps = steps
                .into_iter()
                .map(|step| match (step.error, step.skipped) {
                    (None, None) => {
                        view! { <li class="removed">{format!("✓ {}", step.title)}</li> }.into_any()
                    }
                    (None, Some(reason)) => view! {
                        <li class="skipped">
                            {format!("– {}", step.title)}
                            <div class="note">{format!("Skipped: {reason}")}</div>
                        </li>
                    }
                    .into_any(),
                    (Some(error), _) => view! {
                        <li class="failed">
                            {format!("✕ {}", step.title)}
                            <div class="why">{error}</div>
                        </li>
                    }
                    .into_any(),
                })
                .collect_view();
            view! {
                <h2>{title}</h2>
                <ul class="uninstall-steps">{steps}</ul>
                <p>{text}{removal_line(&plan.removal)}</p>
                <div class="buttons">
                    <button class="primary" on:click=call(on_quit)>
                        {quit_label(&plan.removal)}
                    </button>
                </div>
            }
            .into_any()
        }
    }
}
