//! `styles/tokens.css` is kept by hand and has to agree with
//! `website/brand/brand.json`, which is the palette source of truth.
//!
//! This test is what "derived from the brand" is allowed to mean for a file
//! nothing generates: every palette value in the brand, in both themes, must
//! appear in the stylesheet under a token named after it, and no token may hold
//! a value the brand does not state. A retouched palette that has not reached
//! the window fails here rather than looking slightly wrong in it.

use std::collections::BTreeMap;

const TOKENS: &str = include_str!("../styles/tokens.css");
const BRAND: &str = include_str!("../../../website/brand/brand.json");

/// The stylesheet with its comments removed. They group the tokens for a
/// reader, and would otherwise attach themselves to the declaration that
/// follows.
fn uncommented() -> String {
    let mut plain = String::with_capacity(TOKENS.len());
    let mut rest = TOKENS;
    while let Some((before, after)) = rest.split_once("/*") {
        plain.push_str(before);
        rest = after.split_once("*/").map_or("", |(_, after)| after);
    }
    plain.push_str(rest);
    plain
}

/// The custom properties declared in every block whose selector list contains
/// `selector`.
fn declared_under(selector: &str) -> BTreeMap<String, String> {
    let stylesheet = uncommented();
    let mut declared = BTreeMap::new();
    for block in stylesheet.split('}') {
        let Some((selectors, body)) = block.split_once('{') else {
            continue;
        };
        if !selectors.contains(selector) {
            continue;
        }
        for declaration in body.split(';') {
            let Some((name, value)) = declaration.split_once(':') else {
                continue;
            };
            if let Some(token) = name.trim().strip_prefix("--") {
                declared.insert(token.to_owned(), value.trim().to_owned());
            }
        }
    }
    declared
}

/// The custom properties a document in `theme` ends up with.
fn declared(theme: &str) -> BTreeMap<String, String> {
    declared_under(&format!("[data-theme=\"{theme}\"]"))
}

/// `onAccent` names the token `accent-on-accent`, following the brand's own
/// shape rather than inventing a second one.
fn kebab(camel: &str) -> String {
    let mut kebab = String::with_capacity(camel.len() + 2);
    for (index, character) in camel.char_indices() {
        if character.is_ascii_uppercase() {
            if index != 0 {
                kebab.push('-');
            }
            kebab.push(character.to_ascii_lowercase());
        } else {
            kebab.push(character);
        }
    }
    kebab
}

/// Every `<group>.<key>: value` in one of the brand's palettes, as the token
/// name it should be declared under. `None` when the brand file is not the
/// shape this test knows, which is a thing to report rather than assert past.
fn brand_tokens(theme: &str) -> Option<BTreeMap<String, String>> {
    let brand: serde_json::Value = serde_json::from_str(BRAND).ok()?;
    let palette = brand.get("palette")?.get(theme)?.as_object()?;

    let mut tokens = BTreeMap::new();
    for (group, entries) in palette {
        for (key, value) in entries.as_object()? {
            let value = value.as_str()?;
            tokens.insert(format!("{group}-{}", kebab(key)), value.to_owned());
        }
    }
    Some(tokens)
}

// The comparisons below wrap the stylesheet's side in `Some` rather than
// unwrapping the brand's: a brand file that has changed shape then reads as
// `right: None` instead of a panic, which is the same failure with more of the
// evidence in it.

#[test]
fn both_palettes_reach_the_window_exactly_as_the_brand_states_them() {
    for theme in ["dark", "light"] {
        assert_eq!(
            Some(declared(theme)),
            brand_tokens(theme),
            "the {theme} tokens in styles/tokens.css disagree with brand.json"
        );
    }
}

#[test]
fn the_dark_palette_is_also_what_a_document_with_no_theme_yet_gets() {
    // The theme is applied by the frontend, which cannot run before the first
    // paint. `:root` holding the dark values is what keeps that first frame from
    // being a white flash.
    assert_eq!(Some(declared_under(":root")), brand_tokens("dark"));
}

#[test]
fn the_two_themes_define_the_same_tokens_so_no_component_can_work_in_only_one() {
    let dark: Vec<_> = declared("dark").into_keys().collect();
    let light: Vec<_> = declared("light").into_keys().collect();
    assert_eq!(dark, light);
    assert!(!dark.is_empty(), "the stylesheet parsed to nothing");
}

#[test]
fn every_token_the_config_hands_to_tailwind_is_one_the_stylesheet_declares() {
    // The other half of the derivation: `tailwind.config.js` names tokens by
    // `var(--x)`, and a typo there is a colour that silently resolves to
    // nothing. Every one of them has to exist.
    const CONFIG: &str = include_str!("../tailwind.config.js");

    let declared = declared("dark");
    let mut referenced = 0;
    for (index, _) in CONFIG.match_indices("var(--") {
        let rest = &CONFIG[index + "var(--".len()..];
        let Some(token) = rest.split(')').next() else {
            continue;
        };
        assert!(
            declared.contains_key(token),
            "tailwind.config.js asks for --{token}, which styles/tokens.css does not declare"
        );
        referenced += 1;
    }
    assert!(referenced > 0, "the config referenced no tokens at all");
}
