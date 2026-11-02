use epik_ui::settings::{UninstallBody, UninstallButton, Uninstalling};
use epik_view::{Removal, Uninstall, UninstallStep};
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

/// How to remove the app, for each `Removal`.
const MAC_APP: &str = "drag Epik from Applications to the Trash.";
const MAC_APP_ELSEWHERE: &str = "drag Epik from /Users/me/Downloads to the Trash.";
const WINDOWS_APP: &str = "open Settings › Apps › Installed apps, and uninstall Epik there.";
const APP_IMAGE: &str = "delete the AppImage.";
const DEB: &str = "run <code>sudo apt remove epik</code>.";

/// What the confirmation says about the two steps, ending with `line`.
fn afterward(line: &str) -> String {
    format!(
        "<p>Uninstalling has two steps. First Epik removes its own files, listed above. \
         Then you quit Epik and remove the app the way you'd remove any app: {line}</p>"
    )
}

fn mac_app() -> Removal {
    Removal::MacApp("/Applications/Epik.app".into())
}

fn app_image() -> Removal {
    Removal::AppImage("/home/me/Apps/Epik.AppImage".into())
}

/// Each `Removal`, with how the dialog says to remove the app, and what
/// its Done button says.
fn removals() -> Vec<(Removal, &'static str, &'static str)> {
    vec![
        (mac_app(), MAC_APP, "Quit and Show in Finder"),
        (
            Removal::MacApp("/Users/me/Downloads/Epik.app".into()),
            MAC_APP_ELSEWHERE,
            "Quit and Show in Finder",
        ),
        (
            Removal::WindowsApp,
            WINDOWS_APP,
            "Quit and Open Installed Apps",
        ),
        (app_image(), APP_IMAGE, "Quit and Show the AppImage"),
        (Removal::Deb, DEB, "Quit"),
    ]
}

#[test]
fn settings_ends_with_the_uninstall_button() {
    let html = render(|| view! { <UninstallButton/> });
    assert_eq!(
        html,
        r#"<section class="uninstall"><h3>Uninstall</h3><button class="danger-outline">Uninstall Epik…</button></section>"#
    );
}

#[test]
fn the_confirmation_lists_what_goes_and_what_stays_and_how_to_remove_the_app() {
    let plan = Uninstall {
        runs: 0,
        building: false,
        removal: mac_app(),
    };
    let html = render(|| view! { <UninstallBody plan stage=Uninstalling::Asking(None)/> });
    assert_eq!(
        html,
        [
            r#"<h2>Uninstall Epik?</h2>"#,
            REMOVES,
            KEEPS,
            &afterward(MAC_APP),
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
        removal: Removal::Deb,
    };
    let html = render(|| view! { <UninstallBody plan stage=Uninstalling::Asking(None)/> });
    assert!(
        html.contains(&format!(
            r#"{KEEPS}{}<p class="warning">The history of 3 runs will be deleted and can't be recovered.</p><div class="buttons">"#,
            afterward(DEB)
        )),
        "{html}"
    );
}

#[test]
fn the_confirmation_of_one_run_says_one_run() {
    let plan = Uninstall {
        runs: 1,
        building: false,
        ..Uninstall::default()
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
        removal: Removal::WindowsApp,
    };
    let html = render(|| view! { <UninstallBody plan stage=Uninstalling::Asking(None)/> });
    assert!(
        html.ends_with(&[
            &afterward(WINDOWS_APP),
            r#"<p class="warning">The history of 1 run will be deleted and can't be recovered.</p>"#,
            r#"<p class="why">Wait for the running build to finish, or stop it first.</p>"#,
            r#"<div class="buttons"><button>Cancel</button><button disabled class="danger">Uninstall</button></div>"#,
        ]
        .concat()),
        "{html}"
    );
}

#[test]
fn the_confirmation_says_how_to_remove_the_app_on_each_platform() {
    for (removal, line, _) in removals() {
        let plan = Uninstall {
            removal,
            ..Uninstall::default()
        };
        let html = render(|| view! { <UninstallBody plan stage=Uninstalling::Asking(None)/> });
        assert!(html.contains(&afterward(line)), "{html}");
    }
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
        skipped: None,
    }
}

fn skipped(title: &str, reason: &str) -> UninstallStep {
    UninstallStep {
        skipped: Some(reason.into()),
        ..step(title, None)
    }
}

#[test]
fn when_done_the_dialog_reports_each_step_and_offers_to_remove_the_app() {
    let steps = vec![step("The Epik plugin", None), step("Run history", None)];
    let plan = Uninstall {
        removal: mac_app(),
        ..Uninstall::default()
    };
    let html = render(|| view! { <UninstallBody plan stage=Uninstalling::Done(steps)/> });
    assert_eq!(
        html,
        concat!(
            r#"<h2>Epik's files are removed</h2><ul class="uninstall-steps">"#,
            r#"<li class="removed">✓ The Epik plugin</li>"#,
            r#"<li class="removed">✓ Run history</li></ul>"#,
            r#"<p>One step left: quit Epik, then drag Epik from Applications to the Trash.</p>"#,
            r#"<div class="buttons"><button class="primary">Quit and Show in Finder</button></div>"#,
        )
    );
}

#[test]
fn when_done_the_dialog_says_how_to_remove_the_app_on_each_platform() {
    for (removal, line, button) in removals() {
        let steps = vec![step("Run history", None)];
        let plan = Uninstall {
            removal,
            ..Uninstall::default()
        };
        let html = render(|| view! { <UninstallBody plan stage=Uninstalling::Done(steps)/> });
        assert!(
            html.ends_with(&format!(
                r#"<p>One step left: quit Epik, then {line}</p><div class="buttons"><button class="primary">{button}</button></div>"#
            )),
            "{html}"
        );
    }
}

#[test]
fn when_partly_done_the_dialog_says_how_to_remove_the_app_on_each_platform() {
    for (removal, line, button) in removals() {
        let steps = vec![step("Run history", Some("disk full"))];
        let plan = Uninstall {
            removal,
            ..Uninstall::default()
        };
        let html = render(|| view! { <UninstallBody plan stage=Uninstalling::Done(steps)/> });
        assert!(
            html.ends_with(&format!(
                r#"<p>Epik couldn't remove everything. Remove what failed yourself, then quit Epik and {line}</p><div class="buttons"><button class="primary">{button}</button></div>"#
            )),
            "{html}"
        );
    }
}

#[test]
fn a_failed_step_is_shown_with_its_error() {
    let steps = vec![
        step("The Epik plugin", Some("disk full")),
        step("Run history", None),
    ];
    let plan = Uninstall {
        removal: Removal::Deb,
        ..Uninstall::default()
    };
    let html = render(|| view! { <UninstallBody plan stage=Uninstalling::Done(steps)/> });
    assert_eq!(
        html,
        concat!(
            r#"<h2>Epik is partly uninstalled</h2><ul class="uninstall-steps">"#,
            r#"<li class="failed">✕ The Epik plugin<div class="why">disk full</div></li>"#,
            r#"<li class="removed">✓ Run history</li></ul>"#,
            r#"<p>Epik couldn't remove everything. Remove what failed yourself, then quit Epik and run <code>sudo apt remove epik</code>.</p>"#,
            r#"<div class="buttons"><button class="primary">Quit</button></div>"#,
        )
    );
}

#[test]
fn a_skipped_step_is_shown_with_why_and_uninstalling_still_succeeds() {
    let steps = vec![
        skipped("The Epik plugin", "Claude Code isn't installed"),
        step("Run history", None),
    ];
    let plan = Uninstall {
        removal: Removal::Deb,
        ..Uninstall::default()
    };
    let html = render(|| view! { <UninstallBody plan stage=Uninstalling::Done(steps)/> });
    assert_eq!(
        html,
        concat!(
            r#"<h2>Epik's files are removed</h2><ul class="uninstall-steps">"#,
            r#"<li class="skipped">– The Epik plugin<div class="note">Skipped: Claude Code isn't installed</div></li>"#,
            r#"<li class="removed">✓ Run history</li></ul>"#,
            r#"<p>One step left: quit Epik, then run <code>sudo apt remove epik</code>.</p>"#,
            r#"<div class="buttons"><button class="primary">Quit</button></div>"#,
        )
    );
}
