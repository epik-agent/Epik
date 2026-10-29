use epik_ui::Route;

#[test]
fn the_popover_is_at_its_hash() {
    assert_eq!(Route::from_hash("#/popover"), Some(Route::Popover));
}

#[test]
fn the_setup_checklist_is_at_its_hash() {
    assert_eq!(Route::from_hash("#/setup"), Some(Route::Setup));
}

#[test]
fn other_hashes_are_not_yet_ported() {
    assert_eq!(Route::from_hash("#/"), None);
    assert_eq!(Route::from_hash(""), None);
}

#[test]
fn each_window_has_its_own_stylesheet() {
    assert_eq!(Route::Popover.stylesheet(), "popover.css");
    assert_eq!(Route::Setup.stylesheet(), "setup.css");
}
