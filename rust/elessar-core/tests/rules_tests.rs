//! Phase 2: rules must reproduce the Python engine's hits, strengths and activations.
//! Everything upstream runs for real: parse, fragments, resolution with the golden's committed merges.
mod common;

use elessar_core::pipeline::{fragments, parse};
use elessar_core::resolve::resolve;
use elessar_core::rules::{evaluate, strength, Context};
use elessar_core::store::Table;
use indexmap::IndexMap;

#[test]
fn signals_match_the_python_engine() {
    for id in common::pack_ids() {
        let p = common::pack(&id);
        let g = common::golden("board", &id);

        let mut tables: IndexMap<String, Table> = IndexMap::new();
        let mut frags = Vec::new();
        for sid in &p.source_order {
            let table = parse(&p.sources[sid], common::read_source(&p, sid)).unwrap();
            let maps: Vec<(usize, &elessar_core::pack::Mapping)> = p.mappings.iter().enumerate().filter(|(_, m)| &m.source == sid).collect();
            frags.extend(fragments(&table, &maps));
            tables.insert(sid.clone(), table);
        }
        let mut frags_by_record: IndexMap<(String, u32), Vec<String>> = IndexMap::new();
        for f in &frags {
            frags_by_record.entry((f.source.clone(), f.record)).or_default().push(f.id.clone());
        }
        let committed: Vec<(String, String)> = g["merges_committed"].as_array().unwrap().iter().map(|m| {
            let (a, b) = m.as_str().unwrap().split_once('|').unwrap();
            (a.to_string(), b.to_string())
        }).collect();
        let resolution = resolve(&frags, &p.object_types, &committed, &p.source_order);
        let ctx = Context { tables: &tables, frags_by_record: &frags_by_record, resolution: &resolution };

        for sig in &p.signals {
            let ctx_name = format!("{id} {}", sig.id);
            let want = &g["signals"][sig.id.as_str()];
            let hits = evaluate(sig, &ctx);

            let got_ptrs: Vec<Vec<(&str, &str)>> = hits.iter().map(|h| {
                h.records.iter().map(|(s, i)| (s.as_str(), tables[s].get(*i).unwrap().pointer.as_str())).collect()
            }).collect();
            let want_ptrs: Vec<Vec<(&str, &str)>> = want["hits"].as_array().unwrap().iter().map(|h| {
                h.as_array().unwrap().iter().map(|r| (r["source"].as_str().unwrap(), r["pointer"].as_str().unwrap())).collect()
            }).collect();
            assert_eq!(got_ptrs, want_ptrs, "{ctx_name}: hits");

            let s = strength(sig, &hits, &ctx);
            assert!(common::close(s, want["strength"].as_f64().unwrap()), "{ctx_name}: strength {s}, want {}", want["strength"]);

            let conf = sig.required_sources().iter().map(|r| p.sources[r].confidence).fold(f64::INFINITY, f64::min);
            assert!(common::close(conf, want["confidence"].as_f64().unwrap()), "{ctx_name}: confidence {conf}, want {}", want["confidence"]);
            let activation = if hits.is_empty() { 0.0 } else { conf * s };
            assert!(common::close(activation, want["activation"].as_f64().unwrap()), "{ctx_name}: activation {activation}, want {}", want["activation"]);
        }
    }
}
