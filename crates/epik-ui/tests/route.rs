use epik_ui::Route;

#[test]
fn the_popover_is_at_its_hash() {
    assert_eq!(Route::from_hash("#/popover"), Some(Route::Popover));
}

#[test]
fn other_hashes_are_not_yet_ported() {
    assert_eq!(Route::from_hash("#/setup"), None);
    assert_eq!(Route::from_hash("#/"), None);
    assert_eq!(Route::from_hash(""), None);
}
