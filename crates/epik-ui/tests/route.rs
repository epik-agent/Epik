use epik_ui::Route;
use epik_ui::main_window::{Page, href};

#[test]
fn the_popover_is_at_its_hash() {
    assert_eq!(Route::from_hash("#/popover"), Route::Popover);
}

#[test]
fn the_setup_checklist_is_at_its_hash() {
    assert_eq!(Route::from_hash("#/setup"), Route::Setup);
}

#[test]
fn every_other_hash_is_the_main_window() {
    assert_eq!(Route::from_hash("#/"), Route::Main);
    assert_eq!(Route::from_hash(""), Route::Main);
    assert_eq!(Route::from_hash("#/run/acme/widgets%2313"), Route::Main);
}

#[test]
fn each_window_has_its_own_stylesheet() {
    assert_eq!(Route::Popover.stylesheet(), "popover.css");
    assert_eq!(Route::Setup.stylesheet(), "setup.css");
    assert_eq!(Route::Main.stylesheet(), "app.css");
}

#[test]
fn home_is_at_the_root() {
    assert_eq!(Page::from_hash("#/"), Page::Home);
    assert_eq!(Page::from_hash(""), Page::Home);
    assert_eq!(Page::from_hash("#"), Page::Home);
}

#[test]
fn a_runs_page_is_at_its_key_with_the_hash_escaped() {
    assert_eq!(
        Page::from_hash("#/run/acme/widgets%2313"),
        Page::Run("acme/widgets#13".into())
    );
}

#[test]
fn a_runs_page_hash_is_decoded() {
    assert_eq!(
        Page::from_hash("#/run/acme%2Fwidgets%2313"),
        Page::Run("acme/widgets#13".into())
    );
}

#[test]
fn a_href_escapes_a_runs_hash() {
    assert_eq!(href("run/acme/widgets#13"), "#/run/acme/widgets%2313");
    assert_eq!(href(""), "#/");
}
