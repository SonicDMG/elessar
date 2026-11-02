//! Phase 1b: parsers must reproduce the Python engine's records and skipped rows exactly.
mod common;

use elessar_core::jevcache;
use elessar_core::pack::{Mapping, Source, TimeConfig};
use elessar_core::pipeline::{self, fragments, parse, py};
use elessar_core::time;
use serde_json::{json, Value};

#[test]
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
fn source_checks_warn_about_the_planted_dirt() {
    let p = common::pack("fraud-ring");
    let warnings = elessar_core::pipeline::check_sources(&p).unwrap();
    for needle in ["'orders': 1 row(s) skipped", "'payments': 1 row(s) skipped", "'chargebacks': 1 row(s) skipped",
                   "11 record(s) of 'orders' have a time that does not parse", "2 record(s) of 'tickets' have no time value",
                   "4 record(s) of 'chargebacks' have a time that does not parse", "3 record(s) of 'partner' have no time value"] {
        assert!(warnings.iter().any(|w| w.contains(needle)), "missing warning containing {needle:?}; got {warnings:#?}");
    }
}

#[test]
fn every_pack_passes_the_source_checks() {
    for id in common::pack_ids() {
        pipeline::check_sources(&common::pack(&id)).unwrap_or_else(|e| panic!("{id}: {e:?}"));
    }
}

#[test]
fn a_source_that_fails_as_a_whole_is_an_error() {
    let source = |format: &str, options: Value| -> Source {
        serde_json::from_value(json!({"id": "s", "format": format, "options": options})).unwrap()
    };
    for (format, options, raw) in [
        ("nested-json", json!({"records": "a"}), "{"),
        ("columnar", json!({}), "not json"),
        ("xml", json!({"records": "a"}), "<r><a></r>"),
        ("xml", json!({"records": "a"}), "<r/><r/>"),
        ("xml", json!({"records": "a"}), "<r>&nope;</r>"),
        ("xml", json!({"records": "a"}), "<p:r/>"),
        ("xml", json!({"records": "a"}), ""),
        ("csv", json!({"delimiter": "ab"}), "a,b"),
    ] {
        assert!(parse(&source(format, options), raw.to_string()).is_err(), "{format}: {raw}");
    }
}

#[test]
fn fragments_follow_mapping_order_and_skip_what_python_skips() {
    let s: Source = serde_json::from_value(json!({"id": "src", "format": "jsonl"})).unwrap();
    let raw = "{\"id\": \"A\", \"n\": \"x\", \"k\": 1, \"e\": \"\", \"z\": null}\n{\"id\": \"B\", \"n\": \"7.5\", \"k\": 2}\n";
    let table = parse(&s, raw.to_string()).unwrap();
    let m: Mapping = serde_json::from_value(json!({
        "source": "src", "type": "Thing", "when": {"k": {"gte": 1}},
        "props": {"key": {"from": "id", "transform": ["lower", "upper", "lower"]}, "count": {"from": "n", "transform": "int"}, "blank": "e", "null": "z", "gone": "nope"}
    })).unwrap();
    let only_b: Mapping = serde_json::from_value(json!({"source": "src", "type": "Other", "when": {"k": 2}, "props": {"key": "id"}})).unwrap();
    let nothing: Mapping = serde_json::from_value(json!({"source": "src", "type": "None", "props": {"gone": "nope"}})).unwrap();
    let frags = fragments(&table, &[(3, &m), (7, &only_b), (9, &nothing)]);
    let ids: Vec<&str> = frags.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["src#0:Thing:3", "src#1:Thing:3", "src#1:Other:7"]);
    // A transform that fails (int("x")) drops that property only.
    let keys: Vec<&str> = frags[0].props.keys().map(String::as_str).collect();
    assert_eq!(keys, ["key"]);
    let keys: Vec<&str> = frags[1].props.keys().map(String::as_str).collect();
    assert_eq!(keys, ["key", "count"]);
    assert_eq!(frags[1].props["key"].value, json!("b"));
    assert_eq!(frags[1].props["count"].value, json!(7));
    assert_eq!(frags[1].props["count"].field, "n");
    assert_eq!(frags[1].pointer, "line 2");
}

#[test]
fn every_cached_payload_hashes_to_its_own_file() {
    let dir = common::repo_root().join(".cache").join("jev");
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let mut checked = 0;
    for path in entries.filter_map(|e| e.ok()).map(|e| e.path()) {
        if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
        let entry = jevcache::load_cached(&path).unwrap_or_else(|| panic!("unreadable {}", path.display()));
        let Some(payload) = entry.get("payload") else { continue };
        assert_eq!(jevcache::cache_path(&dir, payload), path);
        checked += 1;
    }
    eprintln!("{checked} cached payload(s) checked");
}

/// Compare against cases dumped from Python, when `ELESSAR_DIFF_CASES` names
/// a file of them: `{parse: [{format, options, raw, records | error, skipped}],
/// matches: [{data, where, want}], transform: [{name, value, want | error}],
/// time: [{value, unit, want | error}], canonical: [{payload, want}]}`.
#[test]
fn differential_cases_from_python() {
    let Ok(path) = std::env::var("ELESSAR_DIFF_CASES") else { return };
    let cases = py::json_loads(&std::fs::read_to_string(path).unwrap()).unwrap();
    let list = |key: &str| cases[key].as_array().cloned().unwrap_or_default();
    let mut failures = Vec::new();
    for c in list("parse") {
        let s: Source = serde_json::from_value(json!({"id": "s", "format": c["format"], "options": c["options"]})).unwrap();
        let raw = c["raw"].as_str().unwrap();
        let got = parse(&s, raw.to_string());
        let got_v = match &got {
            Err(_) => json!({"error": true}),
            Ok(t) => json!({
                "records": t.records.iter().map(|r| json!({"index": r.index, "pointer": r.pointer, "excerpt": t.excerpt(r), "data": r.data})).collect::<Vec<_>>(),
                "skipped": t.skipped.iter().map(|x| json!({"pointer": x.pointer, "reason": x.reason, "excerpt": x.excerpt})).collect::<Vec<_>>(),
            }),
        };
        let want_v = if c.get("error").is_some() { json!({"error": true}) } else { json!({"records": c["records"], "skipped": c["skipped"]}) };
        if got_v != want_v {
            failures.push(format!("parse {} {:?}:\n  got  {got_v}\n  want {want_v}{}", c["format"], raw, got.err().map(|e| format!(" ({e})")).unwrap_or_default()));
        }
    }
    for c in list("matches") {
        let got = pipeline::matches(&c["data"], Some(&c["where"]));
        if Some(got) != c["want"].as_bool() {
            failures.push(format!("matches {} {}: got {got}", c["data"], c["where"]));
        }
    }
    for c in list("transform") {
        let got = pipeline::transform(c["name"].as_str().unwrap(), &c["value"]);
        let same = match (&got, c.get("want")) {
            (Ok(v), Some(w)) => py::dumps(v, false) == py::dumps(w, false),
            (Err(_), None) => true,
            _ => false,
        };
        if !same {
            failures.push(format!("transform {} {}: got {got:?}, want {:?}", c["name"], c["value"], c.get("want")));
        }
    }
    for c in list("time") {
        let got = time::parse_time(&c["value"], c["unit"].as_str().unwrap());
        let same = match (&got, c.get("want")) {
            (Ok(t), Some(w)) => w.as_i64() == Some(*t) && c["iso"].as_str() == Some(&time::iso(*t)),
            (Err(e), None) => Some(e.as_str()) == c["error"].as_str(),
            _ => false,
        };
        if !same {
            failures.push(format!("time {} {}: got {got:?}, want {:?} / {:?}", c["value"], c["unit"], c.get("want"), c.get("error")));
        }
    }
    for c in list("canonical") {
        let got = jevcache::canonical(&c["payload"]);
        if Some(got.as_str()) != c["want"].as_str() {
            failures.push(format!("canonical: got {got}, want {}", c["want"]));
        }
    }
    for c in list("packs") {
        let id = c["id"].as_str().unwrap();
        let p = common::pack(id);
        let mut got = p.warnings.clone();
        got.extend(pipeline::check_sources(&p).unwrap());
        let want: Vec<String> = c["warnings"].as_array().unwrap().iter().map(|w| w.as_str().unwrap().to_string()).collect();
        if got != want {
            failures.push(format!("warnings of {id}:\n  got  {got:#?}\n  want {want:#?}"));
        }
    }
    assert!(failures.is_empty(), "{} failure(s):\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn document_times_cover_every_form() {
    let time_of = |format: &str, doc: Value, unit: &str, raw: &str| {
        let s: Source = serde_json::from_value(json!({"id": "s", "format": format})).unwrap();
        let t: TimeConfig = serde_json::from_value(json!({"document": doc, "unit": unit})).unwrap();
        time::document_time(raw, &s, &t)
    };
    let want = Ok(1_792_022_400_000);
    assert_eq!(time_of("kv", json!({"constant": "2026-10-15"}), "iso", ""), want);
    assert_eq!(time_of("kv", json!({"regex": r"generated (\S+)"}), "iso", "# generated 2026-10-15T00:00:00Z\n"), want);
    assert_eq!(time_of("nested-json", json!({"path": "meta.at"}), "s", r#"{"meta": {"at": 1792022400}}"#), want);
    assert_eq!(time_of("columnar", json!({"path": "as_of"}), "iso", r#"{"as_of": "2026-10-15"}"#), want);
    assert_eq!(time_of("xml", json!({"path": "@issued"}), "iso", r#"<r issued="2026-10-15"/>"#), want);
    assert_eq!(time_of("xml", json!({"path": "head/at"}), "iso", "<r><head><at> 2026-10-15 </at></head></r>"), want);
    assert_eq!(time_of("kv", json!({"regex": "nothing"}), "iso", "x").unwrap_err(), "document regex 'nothing' found no group in the source text");
    assert_eq!(time_of("kv", json!({"regex": "(nothing)"}), "iso", "x").unwrap_err(), "document regex '(nothing)' found no group in the source text");
    assert_eq!(time_of("xml", json!({"path": "@gone"}), "iso", "<r/>").unwrap_err(), "document path '@gone' is empty");
    assert_eq!(time_of("xml", json!({"path": "empty"}), "iso", "<r><empty/></r>").unwrap_err(), "document path 'empty' is empty");
    assert!(time_of("nested-json", json!({"path": "a"}), "iso", "{").is_err());
    assert!(time_of("kv", json!({"path": "a"}), "iso", "a=1").is_err());
}
