//! Pack discovery with the checks that need the source files parsed (spec §1).
//! `pack::load_pack` does the structural checks; `pipeline::check_sources` adds
//! warnings or invalidates the pack, and the server runs both, as Python's loader does.

use elessar_core::pack::{self, Pack};
use elessar_core::pipeline;
use indexmap::IndexMap;
use std::path::Path;

/// `scenario.discover`: valid packs by id, with their warnings, and the errors
/// of the others by directory name.
pub fn discover(root: &Path) -> (IndexMap<String, Pack>, IndexMap<String, Vec<String>>) {
    let (found, mut failures) = pack::discover(root);
    let mut packs = IndexMap::new();
    for (id, mut p) in found {
        match pipeline::check_sources(&p) {
            Ok(warnings) => {
                p.warnings.extend(warnings);
                packs.insert(id, p);
            }
            Err(errors) => {
                failures.insert(p.dir.file_name().map_or(id, |n| n.to_string_lossy().into_owned()), errors);
            }
        }
    }
    failures.sort_keys();
    (packs, failures)
}

/// Validate every pack and print what `python3 -m server.app --check` prints.
/// Returns whether all of them are valid.
pub fn print_check(scenarios_dir: &Path) -> bool {
    let (packs, failures) = discover(scenarios_dir);
    for (id, p) in &packs {
        println!("OK    {id}: {} sources, {} pieces, board {}x{}", p.sources.len(), p.layout.nodes.len(), p.layout.width, p.layout.height);
        for w in &p.warnings {
            println!("      warning: {w}");
        }
    }
    for (name, errors) in &failures {
        println!("FAIL  {name}");
        for e in errors {
            println!("      {e}");
        }
    }
    failures.is_empty()
}
