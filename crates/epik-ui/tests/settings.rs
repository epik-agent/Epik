use epik_ui::settings::{
    ConfirmBody, ConfirmHostBody, HostForm, Hosts, MonitorForm, Repositories, SettingsPage,
};
use epik_view::{HostRow, HostStatus, Settings, SetupFix};
use leptos::prelude::*;

mod fixtures;
use fixtures::{render, repository};

#[test]
fn settings_lists_each_monitored_repository() {
    let settings = Settings {
        repositories: vec![repository("acme/widgets", vec![], false)],
        hosts: vec![],
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
        hosts: vec![],
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
        hosts: vec![],
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
        hosts: vec![this_computer(true)],
    }));
    let error = RwSignal::new(String::new());
    let host_error = RwSignal::new(String::new());
    let html = render(|| view! { <SettingsPage settings error host_error/> });
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
        html.contains(concat!(
            r#"</form></section><section class="hosts"><h3>Hosts</h3>"#,
            r#"<p class="muted">Builds run on one of these machines: this one, or another you reach with ssh that runs Epik. "#,
            r#"<a href="https://github.com/epik-agent/Epik#build-on-another-machine">How to set up a host</a></p>"#,
            r#"<ul id="hosts"><li class="host">"#,
        )),
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
    let host_error = RwSignal::new(String::new());
    let html = render(|| view! { <SettingsPage settings error host_error/> });
    assert!(
        html.contains(r#"<ul id="repositories"><li class="error">no ~/.epik</li></ul>"#),
        "{html}"
    );
}

/// This computer, the default for builds or not.
fn this_computer(default: bool) -> HostRow {
    HostRow {
        destination: None,
        name: "This computer".into(),
        status: HostStatus::Ready,
        about: "Epik 0.4.0".into(),
        why: None,
        missing: vec![],
        default,
        runs: vec![],
    }
}

/// The host `devbox`, in `status`, saying `about`.
fn devbox(status: HostStatus, about: &str) -> HostRow {
    HostRow {
        destination: Some("devbox".into()),
        name: "devbox".into(),
        status,
        about: about.into(),
        why: None,
        missing: vec![],
        default: false,
        runs: vec![],
    }
}

#[test]
fn this_computer_can_be_made_the_default_but_not_removed() {
    let html = render(|| view! { <Hosts hosts=vec![this_computer(false)]/> });
    assert_eq!(
        html,
        concat!(
            r#"<li class="host"><span class="status-dot"></span>"#,
            r#"<div class="host-text"><div class="host-name">This computer</div><div class="muted">Epik 0.4.0</div></div>"#,
            r#"<div class="host-buttons"><button>Make default</button></div></li>"#,
        )
    );
}

#[test]
fn the_default_host_is_marked_instead_of_offered() {
    let row = HostRow {
        default: true,
        ..devbox(HostStatus::Ready, "Epik 0.4.0 · setup complete · No runs")
    };
    let html = render(|| view! { <Hosts hosts=vec![this_computer(false), row]/> });
    assert!(
        html.contains(concat!(
            r#"<li class="host"><span class="status-dot"></span>"#,
            r#"<div class="host-text"><div class="host-name">devbox</div><div class="muted">Epik 0.4.0 · setup complete · No runs</div></div>"#,
            r#"<div class="host-buttons"><span class="default">Default for builds</span>"#,
            r#"<button>Check again</button><button>Remove</button></div></li>"#,
        )),
        "{html}"
    );
    assert!(html.contains(r#"<button>Make default</button>"#), "{html}");
}

#[test]
fn an_unreachable_host_says_what_ssh_said() {
    let row = HostRow {
        why: Some("ssh: Could not resolve hostname devbox".into()),
        ..devbox(HostStatus::Unreachable, "Unreachable · No runs")
    };
    let html = render(|| view! { <Hosts hosts=vec![row]/> });
    assert!(
        html.starts_with(concat!(
            r#"<li class="host unreachable"><span class="status-x">✕</span>"#,
            r#"<div class="host-text"><div class="host-name">devbox</div><div class="muted">Unreachable · No runs</div>"#,
            r#"<div class="why">ssh: Could not resolve hostname devbox</div></div>"#,
        )),
        "{html}"
    );
}

#[test]
fn a_host_of_another_version_says_which_to_install() {
    let row = HostRow {
        why: Some("devbox runs Epik 0.3.2; install 0.4.x there".into()),
        ..devbox(HostStatus::Mismatch, "Epik 0.3.2 · No runs")
    };
    let html = render(|| view! { <Hosts hosts=vec![row]/> });
    assert!(
        html.starts_with(concat!(
            r#"<li class="host unreachable"><span class="status-x">✕</span>"#,
            r#"<div class="host-text"><div class="host-name">devbox</div><div class="muted">Epik 0.3.2 · No runs</div>"#,
            r#"<div class="why">devbox runs Epik 0.3.2; install 0.4.x there</div></div>"#,
        )),
        "{html}"
    );
}

#[test]
fn a_host_not_checked_yet_has_no_mark() {
    let html = render(
        || view! { <Hosts hosts=vec![devbox(HostStatus::Unchecked, "Not checked yet · No runs")]/> },
    );
    assert!(
        html.starts_with(r#"<li class="host unchecked"><span class="status-none"></span>"#),
        "{html}"
    );
}

#[test]
fn a_hosts_missing_setup_items_show_what_to_run_there() {
    let row = HostRow {
        missing: vec![SetupFix {
            title: "GitHub sign-in".into(),
            detail: "not signed in".into(),
            commands: vec![
                "gh auth login -s project".into(),
                "gh auth setup-git".into(),
            ],
        }],
        ..devbox(
            HostStatus::Incomplete,
            "Epik 0.4.0 · setup incomplete · No runs",
        )
    };
    let html = render(|| view! { <Hosts hosts=vec![row]/> });
    assert!(
        html.starts_with(concat!(
            r#"<li class="host incomplete"><span class="status-warn">!</span>"#,
            r#"<div class="host-text"><div class="host-name">devbox</div><div class="muted">Epik 0.4.0 · setup incomplete · No runs</div>"#,
            r#"<ul class="fixes"><li>GitHub sign-in: not signed in. On devbox, run:"#,
            r#"<pre><code>gh auth login -s project</code><code>gh auth setup-git</code></pre></li></ul></div>"#,
        )),
        "{html}"
    );
}

#[test]
fn the_host_form_shows_its_error_under_the_input() {
    let error = RwSignal::new("\"dev box\" is not an SSH destination".to_string());
    let html = render(|| view! { <HostForm error/> });
    assert_eq!(
        html,
        concat!(
            r#"<form id="add-host"><label for="host">Add a host</label>"#,
            r#"<div class="row"><input id="host" placeholder="devbox or me@build.example.com" autocomplete="off" spellcheck="false">"#,
            r#"<button class="primary">Add</button></div>"#,
            r#"<p id="add-host-error" class="error">"dev box" is not an SSH destination</p></form>"#,
        )
    );
}

#[test]
fn the_confirmation_to_remove_a_host_lists_the_runs_that_will_disappear() {
    let row = HostRow {
        runs: vec![("acme/widgets#13".into(), "Exponentiation".into())],
        ..devbox(HostStatus::Ready, "Epik 0.4.0 · 1 run")
    };
    let html = render(|| view! { <ConfirmHostBody host=row/> });
    assert_eq!(
        html,
        concat!(
            r#"<h2 id="confirm-title">Remove devbox?</h2>"#,
            r#"<p id="confirm-text">Epik will stop checking it, and this run will disappear from the window. The run stays on devbox.</p>"#,
            r#"<ul id="confirm-runs"><li>Exponentiation (acme/widgets#13)</li></ul>"#,
        )
    );
}
