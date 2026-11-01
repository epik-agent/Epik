//! Settings, a page of the main window: the monitored repositories, a form
//! to monitor another, and a button to uninstall Epik.

use std::path::Path;

use epik_view::{Removal, RepositoryRow, Settings, Uninstall, UninstallStep};
use leptos::ev::SubmitEvent;
use leptos::html::Input;
use leptos::prelude::*;
use serde::Serialize;

use crate::tauri;
use crate::words::GithubLink;

/// The Settings page: the monitored repositories, each with its button,
/// the form to monitor another, and the button that opens the confirmation
/// to uninstall Epik. The list is redrawn only when `settings` changes, so
/// the form keeps what is typed in it.
#[component]
pub fn SettingsPage(
    #[prop(into)] settings: Signal<Result<Settings, String>>,
    error: RwSignal<String>,
    #[prop(optional_no_strip)] on_stop: Option<Callback<RepositoryRow>>,
    #[prop(optional_no_strip)] on_monitored: Option<Callback<()>>,
    #[prop(optional_no_strip)] on_open_uninstall: Option<Callback<()>>,
) -> impl IntoView {
    let settings = Memo::new(move |_| settings.get());
    view! {
        <h1 class="page-title">"Settings"</h1>
        <div class="card settings">
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
            <UninstallButton on_open=on_open_uninstall/>
        </div>
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
                .map(|step| match step.error {
                    None => {
                        view! { <li class="removed">{format!("✓ {}", step.title)}</li> }.into_any()
                    }
                    Some(error) => view! {
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
