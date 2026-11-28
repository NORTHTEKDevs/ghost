//! The listing manifests at the repo root carry the version a release
//! publishes: `server.json` goes to the MCP registry, `plugin.json` and
//! `lhm.plugin.json` to the plugin listings. Each must name the version this
//! crate is built as, or a listing advertises a release that is not the one
//! users download.

use serde_json::Value;

fn manifest(name: &str, text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{name} is not valid JSON: {e}"))
}

fn manifests() -> [(&'static str, Value); 3] {
    [
        ("server.json", manifest("server.json", include_str!("../../../server.json"))),
        ("plugin.json", manifest("plugin.json", include_str!("../../../plugin.json"))),
        ("lhm.plugin.json", manifest("lhm.plugin.json", include_str!("../../../lhm.plugin.json"))),
    ]
}

#[test]
fn every_listing_manifest_names_the_built_version() {
    let built = env!("CARGO_PKG_VERSION");
    for (name, m) in manifests() {
        assert_eq!(m["version"].as_str(), Some(built), "{name} version differs from Cargo.toml ({built})");
    }
}

/// The registry's server schema caps `description` at 100 characters, and
/// server.json sits exactly at the cap; one more word fails the publish.
#[test]
fn the_registry_description_fits_the_schema() {
    let [(_, server), ..] = manifests();
    let d = server["description"].as_str().expect("server.json description");
    assert!(!d.trim().is_empty());
    assert!(d.chars().count() <= 100, "server.json description is {} chars (max 100)", d.chars().count());
}
