use epik_ui::settings::{ConfirmBody, MonitorForm, Repositories, SettingsPage};
use epik_view::Settings;
use leptos::prelude::*;

mod fixtures;
use fixtures::{render, repository};

#[test]
fn settings_lists_each_monitored_repository() {
    let settings = Settings {
        repositories: vec![repository("acme/widgets", vec![], false)],
    };
    let html = render(|| view! { <Repositories settings=Ok(settings)/> });
    assert_eq!(
        html,
        r#"<li class="repository"><span class="status-dot"></span><div class="repo-text"><a href="https://github.com/acme/widgets">acme/widgets</a><div class="muted">2 runs · added Oct 20</div></div><button>Stop monitoring</button></li>"#
    );
}

#[test]
fn a_repository_no_longer_on_github_can_be_removed() {
    let settings = Settings {
        repositories: vec![repository("acme/gone", vec![], true)],
    };
    let html = render(|| view! { <Repositories settings=Ok(settings)/> });
    assert_eq!(
        html,
        r#"<li class="repository missing"><span class="status-x">✕</span><div class="repo-text"><a href="https://github.com/acme/gone">acme/gone</a><div class="why">No longer on GitHub. It was deleted, renamed, or you lost access.</div></div><button>Remove</button></li>"#
    );
}

#[test]
fn settings_without_repositories_says_so() {
    let settings = Settings {
        repositories: vec![],
    };
    let html = render(|| view! { <Repositories settings=Ok(settings)/> });
    assert_eq!(html, r#"<li class="muted none">No repositories yet.</li>"#);
}

#[test]
fn settings_that_cannot_be_read_say_why() {
    let html = render(|| view! { <Repositories settings=Err("no ~/.epik".into())/> });
    assert_eq!(html, r#"<li class="error">no ~/.epik</li>"#);
}

#[test]
fn the_confirmation_lists_the_runs_that_will_disappear() {
    let row = repository(
        "acme/widgets",
        vec![
            ("acme/widgets#13", "Exponentiation"),
            ("acme/widgets#40", "Logging"),
        ],
        false,
    );
    let html = render(|| view! { <ConfirmBody repository=row/> });
    assert_eq!(
        html,
        concat!(
            r#"<h2 id="confirm-title">Stop monitoring acme/widgets?</h2>"#,
            r#"<p id="confirm-text">Epik will stop checking it, and these 2 runs will disappear from the window:</p>"#,
            r#"<ul id="confirm-runs"><li>Exponentiation (acme/widgets#13)</li><li>Logging (acme/widgets#40)</li></ul>"#,
        )
    );
}

#[test]
fn the_confirmation_of_one_run_says_this_run() {
    let row = repository(
        "acme/widgets",
        vec![("acme/widgets#13", "Exponentiation")],
        false,
    );
    let html = render(|| view! { <ConfirmBody repository=row/> });
    assert!(
        html.contains("Epik will stop checking it, and this run will disappear from the window:"),
        "{html}"
    );
}

#[test]
fn the_confirmation_of_a_repository_without_runs_says_so() {
    let row = repository("acme/widgets", vec![], false);
    let html = render(|| view! { <ConfirmBody repository=row/> });
    assert!(html.contains(r#"<p id="confirm-text">Epik will stop checking it. It has no runs in the window.</p><ul id="confirm-runs"></ul>"#), "{html}");
}

#[test]
fn the_monitor_form_shows_its_error_under_the_input() {
    let error = RwSignal::new("acme/nope isn't on GitHub".to_string());
    let html = render(|| view! { <MonitorForm error/> });
    assert_eq!(
        html,
        concat!(
            r#"<form id="add"><label for="repo">Monitor another repository</label>"#,
            r#"<div class="row"><input id="repo" placeholder="owner/name" autocomplete="off" spellcheck="false"><button class="primary">Add</button></div>"#,
            r#"<p id="add-error" class="error">acme/nope isn't on GitHub</p></form>"#,
        )
    );
}

#[test]
fn the_settings_page_has_the_repositories_the_form_and_uninstall() {
    let settings = Signal::stored(Ok(Settings {
        repositories: vec![repository("acme/widgets", vec![], false)],
    }));
    let error = RwSignal::new(String::new());
    let html = render(|| view! { <SettingsPage settings error/> });
    assert!(
        html.starts_with(concat!(
            r#"<h1 class="page-title">Settings</h1><div class="card settings">"#,
            r#"<section><h3>Monitored repositories</h3>"#,
            r#"<p class="muted">Epik monitors these repositories. When Epik creates an epic or starts a build in a repository that isn't listed, it adds that repository here.</p>"#,
            r#"<ul id="repositories"><li class="repository">"#,
        )),
        "{html}"
    );
    assert!(
        html.contains(r#"<button>Stop monitoring</button></li></ul><form id="add">"#),
        "{html}"
    );
    assert!(
        html.ends_with(concat!(
            r#"</form></section><section class="uninstall"><h3>Uninstall</h3>"#,
            r#"<button class="danger-outline">Uninstall Epik…</button></section></div>"#,
        )),
        "{html}"
    );
}

#[test]
fn the_settings_page_says_why_it_could_not_read_the_repositories() {
    let settings = Signal::stored(Err("no ~/.epik".to_string()));
    let error = RwSignal::new(String::new());
    let html = render(|| view! { <SettingsPage settings error/> });
    assert!(
        html.contains(r#"<ul id="repositories"><li class="error">no ~/.epik</li></ul>"#),
        "{html}"
    );
}
