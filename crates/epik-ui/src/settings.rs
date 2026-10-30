//! Settings, a dialog over Home: the monitored repositories, and a form to
//! monitor another.

use epik_view::{RepositoryRow, Settings};
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
