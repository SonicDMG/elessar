//! Phase 1b: parsers must reproduce the Python engine's records and skipped rows exactly.
mod common;

use elessar_core::pipeline::parse;

#[test]
#[ignore = "phase 1b: remove when pipeline::parse is implemented"]
fn parsers_match_the_python_records() {
    for id in common::pack_ids() {
        let p = common::pack(&id);
        for (sid, s) in &p.sources {
            if s.transport != "file" { continue; }
            let g = common::golden("records", &format!("{id}/{sid}"));
            let table = parse(s, common::read_source(&p, sid)).unwrap_or_else(|e| panic!("{id}/{sid}: {e}"));
            let want = g["records"].as_array().unwrap();
            assert_eq!(table.records.len(), want.len(), "{id}/{sid}: record count");
            for (r, w) in table.records.iter().zip(want) {
                assert_eq!(r.index as u64, w["index"].as_u64().unwrap(), "{id}/{sid}: index");
                assert_eq!(r.pointer, w["pointer"].as_str().unwrap(), "{id}/{sid}: pointer");
                assert_eq!(table.excerpt(r), w["excerpt"].as_str().unwrap(), "{id}/{sid} {}: excerpt", r.pointer);
                assert_eq!(&r.data, &w["data"], "{id}/{sid} {}: data", r.pointer);
            }
            let skipped = g["skipped"].as_array().unwrap();
            assert_eq!(table.skipped.len(), skipped.len(), "{id}/{sid}: skipped count");
            for (a, b) in table.skipped.iter().zip(skipped) {
                assert_eq!(a.pointer, b["pointer"].as_str().unwrap(), "{id}/{sid}: skipped pointer");
                assert_eq!(a.reason, b["reason"].as_str().unwrap(), "{id}/{sid}: skipped reason");
            }
        }
    }
}

#[test]
#[ignore = "phase 1b: remove when pipeline::check_sources is implemented"]
fn source_checks_warn_about_the_planted_dirt() {
    let p = common::pack("fraud-ring");
    let warnings = elessar_core::pipeline::check_sources(&p).unwrap();
    for needle in ["'orders': 1 row(s) skipped", "'payments': 1 row(s) skipped", "'chargebacks': 1 row(s) skipped",
                   "11 record(s) of 'orders' have a time that does not parse", "4 record(s) of 'tickets' have no time value",
                   "4 record(s) of 'chargebacks' have a time that does not parse", "3 record(s) of 'partner' have no time value"] {
        assert!(warnings.iter().any(|w| w.contains(needle)), "missing warning containing {needle:?}; got {warnings:#?}");
    }
}
