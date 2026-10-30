//! Settings, a dialog over Home: the monitored repositories, a form to
//! monitor another, and a button to uninstall Epik.

use epik_view::{RepositoryRow, Settings, Uninstall, UninstallStep};
use leptos::ev::SubmitEvent;
use leptos::html::Input;
use leptos::prelude::*;
use serde::Serialize;

use crate::tauri;
use crate::words::GithubLink;

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
            <p class="muted">"Remove what Epik put on this computer. You delete the app yourself afterward."</p>
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

/// The dialog that uninstalls Epik: what goes and what stays, then how
/// each step went, then Quit. Uninstall is disabled while a build runs.
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
                    "Epik couldn't remove everything. Remove what failed yourself, then quit Epik and delete the app to finish.",
                )
            } else {
                (
                    "Epik is uninstalled",
                    "Epik's files are removed. Quit Epik and delete the app to finish.",
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
                <p>{text}</p>
                <div class="buttons">
                    <button class="primary" on:click=call(on_quit)>"Quit"</button>
                </div>
            }
            .into_any()
        }
    }
}
