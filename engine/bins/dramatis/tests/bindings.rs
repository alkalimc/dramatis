//! Contract: `ui/src/api.gen.ts` is exactly what the `api` types export.
//!
//! Writes the file every run, so `cargo test -p dramatis --test bindings` is also the way
//! to regenerate it. CI runs it and then `git diff --exit-code ui/src/api.gen.ts`.

use std::path::PathBuf;

#[test]
fn export_bindings() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../ui/src/api.gen.ts");
    api::export_bindings(&path).expect("export TypeScript bindings");
    let first = std::fs::read(&path).unwrap();
    api::export_bindings(&path).unwrap();
    assert_eq!(
        first,
        std::fs::read(&path).unwrap(),
        "export is not deterministic"
    );
}
