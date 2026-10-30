use epik_ui::settings::{UninstallBody, UninstallButton, Uninstalling};
use epik_view::{Uninstall, UninstallStep};
use leptos::prelude::*;

mod fixtures;
use fixtures::render;

const REMOVES: &str = concat!(
    r#"<p>Uninstalling removes:</p><ul class="uninstall-list">"#,
    r#"<li>The Epik plugin and its marketplace, from Claude Code</li>"#,
    r#"<li>Epik's own copies of gh and Node</li>"#,
    r#"<li>Epik's link to the app, and its temporary build clones</li>"#,
    r#"<li>Everything else in ~/.epik, including the run history</li></ul>"#,
);

const KEEPS: &str = concat!(
    r#"<p>It leaves alone what you may also use without Epik:</p><ul class="uninstall-list">"#,
    r#"<li>Claude Code, and your sign-in to it</li>"#,
    r#"<li>gh on your PATH, your sign-in to GitHub, and git pushing with gh</li></ul>"#,
);

#[test]
fn settings_ends_with_the_uninstall_button() {
    let html = render(|| view! { <UninstallButton/> });
    assert_eq!(
        html,
        r#"<section class="uninstall"><h3>Uninstall</h3><p class="muted">Remove what Epik put on this computer. You delete the app yourself afterward.</p><button class="danger-outline">Uninstall Epik…</button></section>"#
    );
}

#[test]
fn the_confirmation_lists_what_goes_and_what_stays() {
    let plan = Uninstall {
        runs: 0,
        building: false,
    };
    let html = render(|| view! { <UninstallBody plan stage=Uninstalling::Asking(None)/> });
    assert_eq!(
        html,
        [
            r#"<h2>Uninstall Epik?</h2>"#,
            REMOVES,
            KEEPS,
            r#"<div class="buttons"><button>Cancel</button><button class="danger">Uninstall</button></div>"#,
        ]
        .concat()
    );
}

#[test]
fn the_confirmation_says_how_many_runs_history_is_deleted() {
    let plan = Uninstall {
        runs: 3,
        building: false,
    };
    let html = render(|| view! { <UninstallBody plan stage=Uninstalling::Asking(None)/> });
    assert!(
        html.contains(&format!(
            r#"{KEEPS}<p class="warning">The history of 3 runs will be deleted and can't be recovered.</p><div class="buttons">"#
        )),
        "{html}"
    );
}

#[test]
fn the_confirmation_of_one_run_says_one_run() {
    let plan = Uninstall {
        runs: 1,
        building: false,
    };
    let html = render(|| view! { <UninstallBody plan stage=Uninstalling::Asking(None)/> });
    assert!(
        html.contains("The history of 1 run will be deleted and can't be recovered."),
        "{html}"
    );
}

#[test]
fn while_a_build_runs_uninstall_is_disabled_and_the_dialog_says_why() {
    let plan = Uninstall {
        runs: 1,
        building: true,
    };
    let html = render(|| view! { <UninstallBody plan stage=Uninstalling::Asking(None)/> });
    assert!(
        html.ends_with(concat!(
            r#"<p class="why">Wait for the running build to finish, or stop it first.</p>"#,
            r#"<div class="buttons"><button>Cancel</button><button disabled class="danger">Uninstall</button></div>"#,
        )),
        "{html}"
    );
}

#[test]
fn a_refusal_is_shown_above_the_buttons() {
    let plan = Uninstall::default();
    let html = render(
        || view! { <UninstallBody plan stage=Uninstalling::Asking(Some("no home directory".into()))/> },
    );
    assert!(
        html.ends_with(concat!(
            r#"<p class="error">no home directory</p>"#,
            r#"<div class="buttons"><button>Cancel</button><button class="danger">Uninstall</button></div>"#,
        )),
        "{html}"
    );
}

#[test]
fn while_removing_the_dialog_has_no_buttons() {
    let html = render(
        || view! { <UninstallBody plan=Uninstall::default() stage=Uninstalling::Removing/> },
    );
    assert_eq!(
        html,
        r#"<h2>Uninstalling Epik…</h2><p class="muted">Removing Epik's files.</p>"#
    );
}

fn step(title: &str, error: Option<&str>) -> UninstallStep {
    UninstallStep {
        title: title.into(),
        error: error.map(Into::into),
    }
}

#[test]
fn when_done_the_dialog_reports_each_step_and_offers_quit() {
    let steps = vec![step("The Epik plugin", None), step("Run history", None)];
    let html = render(
        || view! { <UninstallBody plan=Uninstall::default() stage=Uninstalling::Done(steps)/> },
    );
    assert_eq!(
        html,
        concat!(
            r#"<h2>Epik is uninstalled</h2><ul class="uninstall-steps">"#,
            r#"<li class="removed">✓ The Epik plugin</li>"#,
            r#"<li class="removed">✓ Run history</li></ul>"#,
            r#"<p>Epik's files are removed. Quit Epik and delete the app to finish.</p>"#,
            r#"<div class="buttons"><button class="primary">Quit</button></div>"#,
        )
    );
}

#[test]
fn a_failed_step_is_shown_with_its_error() {
    let steps = vec![
        step("The Epik plugin", Some("disk full")),
        step("Run history", None),
    ];
    let html = render(
        || view! { <UninstallBody plan=Uninstall::default() stage=Uninstalling::Done(steps)/> },
    );
    assert_eq!(
        html,
        concat!(
            r#"<h2>Epik is partly uninstalled</h2><ul class="uninstall-steps">"#,
            r#"<li class="failed">✕ The Epik plugin<div class="why">disk full</div></li>"#,
            r#"<li class="removed">✓ Run history</li></ul>"#,
            r#"<p>Epik couldn't remove everything. Remove what failed yourself, then quit Epik and delete the app to finish.</p>"#,
            r#"<div class="buttons"><button class="primary">Quit</button></div>"#,
        )
    );
}
