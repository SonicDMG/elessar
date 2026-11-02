//! Phase 1c: resolution must reproduce the Python engine's clusters; proposals must include the pinned ones.
mod common;

use elessar_core::pipeline::{fragments, parse, Fragment};
use elessar_core::resolve::{fuzzy_norm, proposals, resolve, similarity};
use elessar_core::store::string_form;
use indexmap::IndexMap;
use std::collections::HashSet;

fn all_fragments(id: &str) -> (elessar_core::pack::Pack, Vec<Fragment>) {
    let p = common::pack(id);
    let mut frags = Vec::new();
    for (sid, s) in &p.sources {
        let table = parse(s, common::read_source(&p, sid)).unwrap();
        let maps: Vec<(usize, &elessar_core::pack::Mapping)> = p.mappings.iter().enumerate().filter(|(_, m)| &m.source == sid).collect();
        frags.extend(fragments(&table, &maps));
    }
    (p, frags)
}

#[test]
fn clusters_match_the_python_engine() {
    for id in common::pack_ids() {
        let (p, frags) = all_fragments(&id);
        let g = common::golden("clusters", &id);
        let res = resolve(&frags, &p.object_types, &[], &p.source_order);
        let want = g["clusters"].as_object().unwrap();
        assert_eq!(res.clusters.len(), want.len(), "{id}: cluster count");
        for (cid, c) in &res.clusters {
            let w = want.get(cid).unwrap_or_else(|| panic!("{id}: unexpected cluster {cid}"));
            assert_eq!(c.otype, w["type"].as_str().unwrap(), "{id} {cid}: type");
            let wf: Vec<&str> = w["fragments"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
            assert_eq!(c.fragments, wf, "{id} {cid}: fragments");
            let ws: Vec<&str> = w["sources"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
            assert_eq!(c.sources, ws, "{id} {cid}: sources");
            assert_eq!(c.title.as_deref(), w["title"].as_str(), "{id} {cid}: title");
            let wc: Vec<&str> = w["conflicts"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
            assert_eq!(c.conflicts, wc, "{id} {cid}: conflicts");
            let wp = w["props"].as_object().unwrap();
            // The golden is dumped with sorted keys, so it pins the set of properties, not their order.
            let mut names: Vec<&String> = c.props.keys().collect();
            names.sort();
            assert_eq!(names, wp.keys().collect::<Vec<_>>(), "{id} {cid}: properties");
            for (name, vals) in &c.props {
                let wv = wp[name].as_array().unwrap();
                assert_eq!(vals.len(), wv.len(), "{id} {cid}.{name}: value count");
                for (v, w) in vals.iter().zip(wv) {
                    assert_eq!(&v.value, &w["value"], "{id} {cid}.{name}: value");
                    let ptrs: Vec<&str> = v.provenance.iter().map(|p| p.pointer.as_str()).collect();
                    let wptrs: Vec<&str> = w["provenance"].as_array().unwrap().iter().map(|p| p["pointer"].as_str().unwrap()).collect();
                    assert_eq!(ptrs, wptrs, "{id} {cid}.{name}: provenance");
                }
            }
        }
    }
}

#[test]
fn pinned_proposals_are_found() {
    for id in common::pack_ids() {
        let (p, frags) = all_fragments(&id);
        let spec_path = p.dir.join("expect.json");
        let Ok(text) = std::fs::read_to_string(&spec_path) else { continue };
        let spec: serde_json::Value = serde_json::from_str(&text).unwrap();
        let res = resolve(&frags, &p.object_types, &[], &p.source_order);
        let by_id: IndexMap<String, Fragment> = frags.iter().map(|f| (f.id.clone(), f.clone())).collect();
        let props = proposals(&res, &by_id, &p.object_types, &HashSet::new());
        for pair in spec["proposals_seen"].as_array().unwrap() {
            let (a, b) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
            let found = props.iter().any(|pr| {
                let (x, y) = (pr.a.value.as_str().unwrap_or(""), pr.b.value.as_str().unwrap_or(""));
                (x == a && y == b) || (x == b && y == a)
            });
            assert!(found, "{id}: proposal {a:?} / {b:?} not found; got {:?}", props.iter().map(|p| (p.a.value.clone(), p.b.value.clone(), p.score)).collect::<Vec<_>>());
        }
    }
}

/// Spec §3: blocking must not miss a pair that reaches the threshold on today's packs.
#[test]
fn blocking_misses_nothing_on_the_packs() {
    for id in common::pack_ids() {
        let (p, frags) = all_fragments(&id);
        let res = resolve(&frags, &p.object_types, &[], &p.source_order);
        let by_id: IndexMap<String, Fragment> = frags.iter().map(|f| (f.id.clone(), f.clone())).collect();
        let found: HashSet<(String, String)> = proposals(&res, &by_id, &p.object_types, &HashSet::new()).into_iter().map(|pr| (pr.a.cluster, pr.b.cluster)).collect();
        let mut want = HashSet::new();
        for (tname, t) in &p.object_types {
            let Some(fz) = &t.fuzzy else { continue };
            let mut cs: Vec<_> = res.clusters.values().filter(|c| &c.otype == tname).collect();
            cs.sort_by(|a, b| a.id.cmp(&b.id));
            let names: Vec<Vec<String>> = cs.iter().map(|c| fz.properties.iter().flat_map(|n| c.props.get(n).into_iter().flatten()).map(|v| fuzzy_norm(&string_form(&v.value))).collect()).collect();
            for i in 0..cs.len() {
                for j in i + 1..cs.len() {
                    if names[i].iter().any(|x| names[j].iter().any(|y| similarity(x, y) >= fz.threshold)) {
                        want.insert((cs[i].id.clone(), cs[j].id.clone()));
                    }
                }
            }
        }
        assert_eq!(found, want, "{id}: proposals against all pairs");
    }
}
