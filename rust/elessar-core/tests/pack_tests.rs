//! Phase 1a (done): every pack loads; validator errors match the Python engine's on broken packs.
mod common;

use elessar_core::pack::{discover, load_pack};

#[test]
fn every_pack_loads_with_the_python_layout() {
    for id in common::pack_ids() {
        let p = common::pack(&id);
        let g = common::golden("board", &id);
        let nodes = g["nodes"].as_object().unwrap();
        assert_eq!(p.layout.nodes.len(), nodes.len(), "{id}: piece count");
        assert_eq!(p.layout.width, 2 * p.signals.len().max(p.participants.len()).max(p.hypotheses.len()) as u32 + 1);
    }
}

#[test]
fn discover_finds_every_pack_and_rejects_the_old_entities_key() {
    let (packs, failures) = discover(&common::repo_root().join("scenarios"));
    assert_eq!(packs.len(), common::pack_ids().len());
    assert!(failures.is_empty(), "{failures:?}");

    let dir = std::env::temp_dir().join(format!("elessar-oldkey-{}", std::process::id()));
    let src = common::repo_root().join("scenarios").join("supply-chain");
    copy_dir(&src, &dir);
    let model = std::fs::read_to_string(dir.join("model.json")).unwrap().replace("\"participants\"", "\"entities\"");
    std::fs::write(dir.join("model.json"), model).unwrap();
    let err = load_pack(&dir).err().expect("old key must fail");
    assert!(err.errors.iter().any(|e| e.contains("renamed to 'participants'")), "{:?}", err.errors);
    std::fs::remove_dir_all(&dir).ok();
}

fn copy_dir(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap().filter_map(|e| e.ok()) {
        let to = dst.join(e.file_name());
        if e.path().is_dir() { copy_dir(&e.path(), &to); } else { std::fs::copy(e.path(), to).unwrap(); }
    }
}
