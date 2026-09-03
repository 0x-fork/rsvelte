//! `getEntryDescription` is ported, not wrapped, so the port is compared to the
//! function itself on every entry `vscode-css-languageservice` ships, plus the
//! ones the official server's own data provider adds. Regenerate the fixture
//! with `node scripts/dev/generate-css-data.mjs`.

use std::collections::BTreeMap;

use serde::Deserialize;

use rsvelte_language_server::css_data::documentation::{Described, documentation};
use rsvelte_language_server::css_data::svelte_css::SVELTE_PSEUDO_CLASSES;
use rsvelte_language_server::css_data::web::{
    AT_DIRECTIVES, BASELINE_HIGH_IMAGE, BASELINE_LIMITED_IMAGE, BASELINE_LOW_IMAGE, PROPERTIES,
    PSEUDO_CLASSES, PSEUDO_ELEMENTS,
};

const ORACLE: &str = include_str!("data/css-documentation.json");

/// The fixture carries a token where upstream inlines ~1.5 KB of base64, so the
/// rows stay readable; the images themselves are pinned below.
fn tokenize(value: String) -> String {
    value
        .replace(BASELINE_LIMITED_IMAGE, "<BASELINE_LIMITED>")
        .replace(BASELINE_LOW_IMAGE, "<BASELINE_LOW>")
        .replace(BASELINE_HIGH_IMAGE, "<BASELINE_HIGH>")
}

#[derive(Deserialize)]
struct Oracle {
    images: BTreeMap<String, String>,
    entries: BTreeMap<String, [Option<String>; 2]>,
}

/// Tokenizing an image on both sides makes a corrupted constant replace itself,
/// so the URIs are compared to upstream's own bytes first.
#[test]
fn the_baseline_images_are_the_ones_upstream_ships() {
    let oracle: Oracle = serde_json::from_str(ORACLE).expect("parse the documentation oracle");
    assert_eq!(oracle.images["BASELINE_LIMITED"], BASELINE_LIMITED_IMAGE);
    assert_eq!(oracle.images["BASELINE_LOW"], BASELINE_LOW_IMAGE);
    assert_eq!(oracle.images["BASELINE_HIGH"], BASELINE_HIGH_IMAGE);
}

#[test]
fn every_entry_documents_itself_the_way_upstream_does() {
    let expected = serde_json::from_str::<Oracle>(ORACLE)
        .expect("parse the documentation oracle")
        .entries;

    let mut actual: BTreeMap<String, [Option<String>; 2]> = BTreeMap::new();
    let mut record = |key: String, entry: Described| {
        let rendered = [
            documentation(&entry, true).map(tokenize),
            documentation(&entry, false).map(tokenize),
        ];
        assert!(
            actual.insert(key.clone(), rendered).is_none(),
            "{key} twice"
        );
    };
    for (index, property) in PROPERTIES.iter().enumerate() {
        record(
            format!("property:{index}:{}", property.name),
            property.into(),
        );
        for (position, value) in property.values.iter().enumerate() {
            record(
                format!(
                    "property:{index}:{}/value:{position}:{}",
                    property.name, value.name
                ),
                value.into(),
            );
        }
    }
    for (kind, entries) in [
        ("at-directive", AT_DIRECTIVES),
        ("pseudo-class", PSEUDO_CLASSES),
        ("pseudo-element", PSEUDO_ELEMENTS),
        ("svelte-pseudo-class", SVELTE_PSEUDO_CLASSES),
    ] {
        for (index, entry) in entries.iter().enumerate() {
            record(format!("{kind}:{index}:{}", entry.name), entry.into());
        }
    }

    // Both sides come from one generator run, so an empty table would make the
    // comparison below true by construction rather than by agreement.
    for (table, len) in [
        ("PROPERTIES", PROPERTIES.len()),
        ("AT_DIRECTIVES", AT_DIRECTIVES.len()),
        ("PSEUDO_CLASSES", PSEUDO_CLASSES.len()),
        ("PSEUDO_ELEMENTS", PSEUDO_ELEMENTS.len()),
        ("SVELTE_PSEUDO_CLASSES", SVELTE_PSEUDO_CLASSES.len()),
        (
            "PROPERTIES[..].values",
            PROPERTIES.iter().map(|p| p.values.len()).sum(),
        ),
    ] {
        assert!(len > 0, "{table} is empty, so this test has no population");
    }

    // `CSSDataManager.collectData` keeps the first entry per name, so a name the
    // shipped data repeats is offered once; both sides of this comparison share
    // the table, and only a distinctness check can see the duplicate.
    for (table, names) in [
        (
            "PROPERTIES",
            PROPERTIES.iter().map(|p| p.name).collect::<Vec<_>>(),
        ),
        (
            "AT_DIRECTIVES",
            AT_DIRECTIVES.iter().map(|e| e.name).collect::<Vec<_>>(),
        ),
        (
            "PSEUDO_CLASSES",
            PSEUDO_CLASSES.iter().map(|e| e.name).collect::<Vec<_>>(),
        ),
        (
            "PSEUDO_ELEMENTS",
            PSEUDO_ELEMENTS.iter().map(|e| e.name).collect::<Vec<_>>(),
        ),
    ] {
        let distinct: std::collections::BTreeSet<&str> = names.iter().copied().collect();
        assert_eq!(distinct.len(), names.len(), "{table} repeats a name");
    }

    assert_eq!(
        actual.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>(),
        "the port and the oracle describe different entries"
    );
    let differing: Vec<&String> = actual
        .iter()
        .filter(|(key, rendered)| expected[*key] != **rendered)
        .map(|(key, _)| key)
        .collect();
    assert!(
        differing.is_empty(),
        "{} of {} entries render differently, first: {:?}\n  port:   {:?}\n  oracle: {:?}",
        differing.len(),
        actual.len(),
        differing[0],
        actual[differing[0]],
        expected[differing[0]],
    );
}
