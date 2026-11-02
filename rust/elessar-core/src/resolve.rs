//! Entity resolution (spec §3, §4). Phase 1c.
//!
//! Built against `golden/clusters/<pack>.json` (clusters, titles, conflicts
//! and provenance pointers dumped from the Python engine, which must match)
//! and the proposals pinned by each pack's `expect.json` (scores may differ:
//! Rust uses Jaro-Winkler, see the spec).

use crate::pack::{Fuzzy, ObjectType};
use crate::pipeline::Fragment;
use crate::store::string_form;
use crate::types::{MergeSide, PropValue, Proposal, Provenance};
use indexmap::IndexMap;
use serde_json::Value;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

/// How many later strings of a block a string is compared with. Blocks are in
/// string order, so a common block (`ltd`, `ion`) still compares the strings
/// closest to each other, and the work stays linear in the number of strings.
const BLOCK_WINDOW: usize = 64;

/// Drop repeated and surrounding spaces. Spaces are the only whitespace the
/// normalisers let through.
fn collapse(chars: impl Iterator<Item = char>) -> String {
    let mut out = String::new();
    for c in chars {
        if c != ' ' || !(out.is_empty() || out.ends_with(' ')) {
            out.push(c);
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// `norm`: lowercase, keep `[0-9a-z@._ -]`, collapse whitespace, trim.
pub fn norm(value: &str) -> String {
    collapse(value.to_lowercase().chars().filter(|c| matches!(c, '0'..='9' | 'a'..='z' | '@' | '.' | '_' | ' ' | '-')))
}

/// `fuzzy_norm`: lowercase, non `[0-9a-z ]` to space, collapse, trim.
pub fn fuzzy_norm(value: &str) -> String {
    collapse(value.to_lowercase().chars().map(|c| if matches!(c, '0'..='9' | 'a'..='z') { c } else { ' ' }))
}

fn norm_value(value: &Value) -> String {
    norm(&string_form(value))
}

fn jaro_winkler<T: PartialEq>(a: &[T], b: &[T]) -> f64 {
    let window = (a.len().max(b.len()) / 2).saturating_sub(1);
    let mut b_used = vec![false; b.len()];
    let mut a_matched = Vec::new();
    for (i, x) in a.iter().enumerate() {
        let end = (i + window + 1).min(b.len());
        for j in i.saturating_sub(window)..end {
            if !b_used[j] && *x == b[j] {
                b_used[j] = true;
                a_matched.push(x);
                break;
            }
        }
    }
    if a_matched.is_empty() {
        return 0.0;
    }
    let b_matched = b.iter().zip(&b_used).filter(|(_, used)| **used).map(|(y, _)| y);
    let half_transpositions = a_matched.iter().zip(b_matched).filter(|(x, y)| **x != *y).count();
    let m = a_matched.len() as f64;
    let t = (half_transpositions / 2) as f64;
    let jaro = (m / a.len() as f64 + m / b.len() as f64 + (m - t) / m) / 3.0;
    let prefix = a.iter().zip(b).take(4).take_while(|(x, y)| x == y).count() as f64;
    jaro + prefix * 0.1 * (1.0 - jaro)
}

/// Jaro-Winkler similarity on normalised strings (prefix scale 0.1, max prefix 4).
pub fn similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    // Greedy matching is not quite symmetric. A fixed order keeps a pair's
    // score stable when a merge changes which cluster comes first.
    let (a, b) = if a < b { (a, b) } else { (b, a) };
    if a.is_ascii() && b.is_ascii() {
        jaro_winkler(a.as_bytes(), b.as_bytes())
    } else {
        jaro_winkler(&a.chars().collect::<Vec<_>>(), &b.chars().collect::<Vec<_>>())
    }
}

#[derive(Debug, Clone)]
pub struct Cluster {
    /// The smallest fragment id in the cluster (string order).
    pub id: String,
    pub otype: String,
    /// Fragment ids in source-rank-then-id order.
    pub fragments: Vec<String>,
    /// Sources in rack order.
    pub sources: Vec<String>,
    pub props: IndexMap<String, Vec<PropValue>>,
    pub title: Option<String>,
    pub conflicts: Vec<String>,
}

pub struct Resolution {
    pub clusters: IndexMap<String, Cluster>,
    /// fragment id -> cluster id
    pub cluster_of: IndexMap<String, String>,
}

/// Union-find over fragment positions. The root of a set is its smallest
/// fragment id, which makes it the cluster id.
struct UnionFind<'a> {
    fragments: &'a [Fragment],
    parent: Vec<usize>,
}

impl UnionFind<'_> {
    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (lo, hi) = if self.fragments[ra].id < self.fragments[rb].id { (ra, rb) } else { (rb, ra) };
            self.parent[hi] = lo;
        }
    }
}

fn build_cluster(members: &[&Fragment], object_types: &IndexMap<String, ObjectType>, id: String) -> Cluster {
    let mut props: IndexMap<String, Vec<PropValue>> = IndexMap::new();
    let mut seen: HashMap<(&str, String), usize> = HashMap::new();
    let mut sources: Vec<String> = Vec::new();
    for f in members {
        if !sources.contains(&f.source) {
            sources.push(f.source.clone());
        }
        for (name, prop) in &f.props {
            let vals = props.entry(name.clone()).or_default();
            let prov = Provenance { source: f.source.clone(), pointer: f.pointer.clone(), field: prop.field.clone(), fragment: f.id.clone(), excerpt: String::new() };
            match seen.entry((name.as_str(), norm_value(&prop.value))) {
                Entry::Occupied(e) => vals[*e.get()].provenance.push(prov),
                Entry::Vacant(e) => {
                    e.insert(vals.len());
                    vals.push(PropValue { value: prop.value.clone(), provenance: vec![prov] });
                }
            }
        }
    }
    let otype = object_types.get(&members[0].otype);
    let title = otype.and_then(|t| t.title.list().into_iter().find_map(|name| props.get(name))).map(|vals| string_form(&vals[0].value));
    let mut conflicts: Vec<String> = props.iter().filter(|(name, vals)| vals.len() > 1 && !otype.is_some_and(|t| t.multi.contains(name))).map(|(name, _)| name.clone()).collect();
    conflicts.sort();
    Cluster { id, otype: members[0].otype.clone(), fragments: members.iter().map(|f| f.id.clone()).collect(), sources, props, title, conflicts }
}

/// Merge fragments by shared key values and committed pairs (spec §4).
/// `source_rank` is rack order. Excerpts for provenance are filled by the caller.
pub fn resolve(fragments: &[Fragment], object_types: &IndexMap<String, ObjectType>, committed: &[(String, String)], source_rank: &[String]) -> Resolution {
    let mut uf = UnionFind { fragments, parent: (0..fragments.len()).collect() };

    let mut seen: HashMap<(&str, &str, String), usize> = HashMap::new();
    for (i, f) in fragments.iter().enumerate() {
        let Some(otype) = object_types.get(&f.otype) else { continue };
        for key in &otype.keys {
            let Some(prop) = f.props.get(key) else { continue };
            match seen.entry((f.otype.as_str(), key.as_str(), norm_value(&prop.value))) {
                Entry::Occupied(e) => uf.union(*e.get(), i),
                Entry::Vacant(e) => {
                    e.insert(i);
                }
            }
        }
    }

    let position: HashMap<&str, usize> = fragments.iter().enumerate().map(|(i, f)| (f.id.as_str(), i)).collect();
    for (a, b) in committed {
        if let (Some(&a), Some(&b)) = (position.get(a.as_str()), position.get(b.as_str())) {
            uf.union(a, b);
        }
    }

    // Clusters come out in the order their first fragment was seen.
    let mut groups: IndexMap<usize, Vec<&Fragment>> = IndexMap::new();
    for (i, f) in fragments.iter().enumerate() {
        groups.entry(uf.find(i)).or_default().push(f);
    }

    let rank: HashMap<&str, usize> = source_rank.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect();
    let mut clusters = IndexMap::with_capacity(groups.len());
    let mut cluster_of = IndexMap::with_capacity(fragments.len());
    for (root, mut members) in groups {
        let id = fragments[root].id.clone();
        members.sort_by_key(|f| (rank.get(f.source.as_str()).copied().unwrap_or(usize::MAX), f.id.as_str()));
        for f in &members {
            cluster_of.insert(f.id.clone(), id.clone());
        }
        clusters.insert(id.clone(), build_cluster(&members, object_types, id));
    }
    Resolution { clusters, cluster_of }
}

/// Where a fuzzy string occurs: positions of the cluster, the configured
/// property and the value within that property.
#[derive(Clone, Copy)]
struct Occurrence {
    cluster: usize,
    prop: usize,
    value: usize,
}

/// The best match so far for a pair of clusters. On equal scores the lowest
/// `order` wins, so the choice does not depend on the order pairs are found in.
struct Match {
    score: f64,
    order: (usize, usize, usize, usize),
    a: Occurrence,
    b: Occurrence,
}

/// What a string is blocked on: its tokens and its 3-grams. `fuzzy_norm`
/// output is ASCII, so byte windows are valid slices.
fn blocking_grams(s: &str) -> impl Iterator<Item = &str> {
    s.split(' ').chain((0..s.len().saturating_sub(2)).map(move |i| &s[i..i + 3]))
}

fn record_matches(best: &mut HashMap<(usize, usize), Match>, xs: &[Occurrence], ys: &[Occurrence], same_string: bool, score: f64) {
    for x in xs {
        for y in ys {
            if x.cluster == y.cluster || (same_string && x.cluster > y.cluster) {
                continue;
            }
            let (a, b) = if x.cluster < y.cluster { (*x, *y) } else { (*y, *x) };
            let order = (a.prop, b.prop, a.value, b.value);
            match best.entry((a.cluster, b.cluster)) {
                Entry::Occupied(mut e) => {
                    let cur = e.get();
                    if score > cur.score || (score == cur.score && order < cur.order) {
                        e.insert(Match { score, order, a, b });
                    }
                }
                Entry::Vacant(e) => {
                    e.insert(Match { score, order, a, b });
                }
            }
        }
    }
}

/// The best match at or above the threshold for each pair of `clusters` (by
/// position) that has one. Work is done per distinct string, so clusters that
/// share a name cost one comparison, and blocks are bounded by `BLOCK_WINDOW`.
fn best_matches(clusters: &[&Cluster], fuzzy: &Fuzzy) -> HashMap<(usize, usize), Match> {
    let mut found: Vec<(String, Occurrence)> = Vec::new();
    for (cluster, c) in clusters.iter().enumerate() {
        for (prop, name) in fuzzy.properties.iter().enumerate() {
            for (value, v) in c.props.get(name).into_iter().flatten().enumerate() {
                let s = fuzzy_norm(&string_form(&v.value));
                if !s.is_empty() {
                    found.push((s, Occurrence { cluster, prop, value }));
                }
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    let mut strings: Vec<String> = Vec::new();
    let mut occurrences: Vec<Vec<Occurrence>> = Vec::new();
    for (s, at) in found {
        if strings.last() != Some(&s) {
            strings.push(s);
            occurrences.push(Vec::new());
        }
        occurrences.last_mut().unwrap().push(at);
    }

    // Members are unique and in string order: strings are visited in that order.
    let mut blocks: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, s) in strings.iter().enumerate() {
        for gram in blocking_grams(s) {
            let block = blocks.entry(gram).or_default();
            if block.last() != Some(&i) {
                block.push(i);
            }
        }
    }

    let mut best = HashMap::new();
    let mut compared_with = vec![usize::MAX; strings.len()];
    for (i, s) in strings.iter().enumerate() {
        if 1.0 >= fuzzy.threshold {
            record_matches(&mut best, &occurrences[i], &occurrences[i], true, 1.0);
        }
        for gram in blocking_grams(s) {
            let block = &blocks[gram];
            let later = block.partition_point(|&j| j <= i);
            for &j in block[later..].iter().take(BLOCK_WINDOW) {
                if compared_with[j] == i {
                    continue;
                }
                compared_with[j] = i;
                let score = similarity(s, &strings[j]);
                if score >= fuzzy.threshold {
                    record_matches(&mut best, &occurrences[i], &occurrences[j], false, score);
                }
            }
        }
    }
    best
}

/// Keys present on both clusters with no shared `norm` value.
fn key_conflicts(a: &Cluster, b: &Cluster, keys: &[String]) -> Vec<String> {
    keys.iter()
        .filter(|k| match (a.props.get(*k), b.props.get(*k)) {
            (Some(xs), Some(ys)) if !xs.is_empty() && !ys.is_empty() => {
                let xs: HashSet<String> = xs.iter().map(|v| norm_value(&v.value)).collect();
                !ys.iter().any(|v| xs.contains(&norm_value(&v.value)))
            }
            _ => false,
        })
        .cloned()
        .collect()
}

fn merge_side(c: &Cluster, fuzzy: &Fuzzy, at: Occurrence, fragments: &IndexMap<String, Fragment>) -> MergeSide {
    let property = &fuzzy.properties[at.prop];
    let v = &c.props[property][at.value];
    let first = &v.provenance[0];
    let source = fragments.get(&first.fragment).map_or(&first.source, |f| &f.source);
    MergeSide { cluster: c.id.clone(), property: property.clone(), value: v.value.clone(), source: source.clone(), fragment: first.fragment.clone() }
}

/// Fuzzy merge proposals between clusters of the same type, excluding rejected ids.
pub fn proposals(res: &Resolution, fragments: &IndexMap<String, Fragment>, object_types: &IndexMap<String, ObjectType>, rejected: &HashSet<String>) -> Vec<Proposal> {
    let mut by_type: HashMap<&str, Vec<&Cluster>> = HashMap::new();
    for c in res.clusters.values() {
        by_type.entry(c.otype.as_str()).or_default().push(c);
    }
    let mut out = Vec::new();
    for (tname, otype) in object_types {
        let (Some(fuzzy), Some(clusters)) = (&otype.fuzzy, by_type.get_mut(tname.as_str())) else { continue };
        clusters.sort_by(|a, b| a.id.cmp(&b.id));
        for (&(ca, cb), m) in &best_matches(clusters, fuzzy) {
            let (a, b) = (clusters[ca], clusters[cb]);
            let (side_a, side_b) = (merge_side(a, fuzzy, m.a, fragments), merge_side(b, fuzzy, m.b, fragments));
            let id = if side_a.fragment < side_b.fragment { format!("{}|{}", side_a.fragment, side_b.fragment) } else { format!("{}|{}", side_b.fragment, side_a.fragment) };
            if rejected.contains(&id) {
                continue;
            }
            out.push(Proposal { id, r#type: tname.clone(), score: (m.score * 1000.0).round() / 1000.0, a: side_a, b: side_b, conflict: key_conflicts(a, b, &otype.keys) });
        }
    }
    out.sort_by(|x, y| (&x.r#type, &x.a.cluster, &x.b.cluster).cmp(&(&y.r#type, &y.a.cluster, &y.b.cluster)));
    out
}

/// The cluster a participant anchors to: the first cluster of its type with a
/// property whose `norm` equals the anchor value.
pub fn anchor<'a>(res: &'a Resolution, otype: &str, prop: &str, value: &str) -> Option<&'a Cluster> {
    let want = norm(value);
    res.clusters.values().find(|c| c.otype == otype && c.props.get(prop).is_some_and(|vals| vals.iter().any(|v| norm_value(&v.value) == want)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &str, b: &str, want: f64) {
        let got = similarity(a, b);
        assert!((got - want).abs() < 5e-5, "{a}/{b}: {got} vs {want}");
    }

    #[test]
    fn norm_keeps_its_class_and_drops_the_rest() {
        assert_eq!(norm("  Ada   LOVELACE!  "), "ada lovelace");
        assert_eq!(norm("A.B_C-D@e.io"), "a.b_c-d@e.io");
        // Dropped, not replaced: a tab or a comma does not separate words.
        assert_eq!(norm("Foo\tBar,Baz"), "foobarbaz");
        assert_eq!(norm("Café"), "caf");
        assert_eq!(norm("!!!"), "");
    }

    #[test]
    fn fuzzy_norm_replaces_with_spaces() {
        assert_eq!(fuzzy_norm("Alder & Finch  Plastics Co."), "alder finch plastics co");
        assert_eq!(fuzzy_norm("Foo\tBar,Baz"), "foo bar baz");
        assert_eq!(fuzzy_norm("a@b.io"), "a b io");
        assert_eq!(fuzzy_norm("Café!"), "caf");
        assert_eq!(fuzzy_norm("  ?? "), "");
    }

    #[test]
    fn similarity_known_values() {
        close("MARTHA", "MARHTA", 0.9611);
        close("DWAYNE", "DUANE", 0.84);
        close("DIXON", "DICKSONX", 0.8133);
        close("abc", "xyz", 0.0);
    }

    #[test]
    fn similarity_edges() {
        assert_eq!(similarity("same", "same"), 1.0);
        assert_eq!(similarity("", "x"), 0.0);
        assert_eq!(similarity("x", ""), 0.0);
        assert_eq!(similarity("", ""), 0.0);
        assert_eq!(similarity("a", "ab"), similarity("ab", "a"));
        close("é", "è", 0.0);
    }

    fn frag(id: &str, source: &str, props: &[(&str, &str)]) -> Fragment {
        let props = props.iter().map(|(k, v)| (k.to_string(), crate::pipeline::FragProp { value: Value::String(v.to_string()), field: format!("f_{k}") })).collect();
        Fragment { id: id.into(), otype: "T".into(), source: source.into(), record: 0, mapping: 0, pointer: format!("at {id}"), props }
    }

    fn resolve_all(fragments: &[Fragment], otypes: &IndexMap<String, ObjectType>, committed: &[(String, String)]) -> Resolution {
        resolve(fragments, otypes, committed, &["first".to_string(), "second".to_string()])
    }

    #[test]
    fn keys_merge_transitively_and_by_norm() {
        let fragments = [frag("a", "first", &[("badge", "B-1")]), frag("c", "first", &[("email", "X@y.io")]), frag("b", "first", &[("badge", "b-1 "), ("email", "x@y.io")]), frag("d", "first", &[("badge", "B-2")])];
        let res = resolve_all(&fragments, &types(&["name"], &["badge", "email"]), &[]);
        assert_eq!(res.clusters.keys().collect::<Vec<_>>(), ["a", "d"]);
        assert_eq!(res.clusters["a"].fragments, ["a", "b", "c"]);
        assert_eq!(res.cluster_of["c"], "a");
    }

    #[test]
    fn a_key_only_merges_within_its_own_property() {
        let fragments = [frag("a", "first", &[("badge", "same")]), frag("b", "first", &[("email", "same")])];
        let res = resolve_all(&fragments, &types(&["name"], &["badge", "email"]), &[]);
        assert_eq!(res.clusters.len(), 2);
    }

    #[test]
    fn committed_pairs_merge_and_unknown_ids_are_ignored() {
        let fragments = [frag("b", "first", &[("name", "One")]), frag("a", "first", &[("name", "Two")]), frag("c", "first", &[("name", "Three")])];
        let committed = [("b".to_string(), "a".to_string()), ("c".to_string(), "gone".to_string())];
        let res = resolve_all(&fragments, &types(&["name"], &[]), &committed);
        assert_eq!(res.clusters.keys().collect::<Vec<_>>(), ["a", "c"]);
        assert_eq!(res.clusters["a"].fragments, ["a", "b"]);
    }

    #[test]
    fn cluster_orders_by_rack_then_id_and_collapses_equal_values() {
        // The smallest id is in the later source: it names the cluster but comes after the rack's first.
        let fragments = [
            frag("a", "second", &[("badge", "B-1"), ("zone", "North"), ("name", "Marcus Vale")]),
            frag("z", "unknown", &[("badge", "B-1"), ("extra", "x")]),
            frag("m", "first", &[("badge", "B-1"), ("name", "MARCUS VALE!"), ("alias", "Vale")]),
            frag("c", "first", &[("badge", "B-1"), ("name", "M. Vale")]),
        ];
        let mut otypes = types(&["name"], &["badge"]);
        otypes["T"].title = crate::pack::Fields::Many(vec!["missing".into(), "name".into(), "badge".into()]);
        otypes["T"].multi = vec!["alias".into(), "zone".into()];
        let res = resolve_all(&fragments, &otypes, &[]);
        let c = &res.clusters["a"];
        assert_eq!(c.fragments, ["c", "m", "a", "z"]);
        assert_eq!(c.sources, ["first", "second", "unknown"]);
        assert_eq!(c.props.keys().collect::<Vec<_>>(), ["badge", "name", "alias", "zone", "extra"]);
        // First seen wins: "c" is first in rack order, and titles follow the configured list.
        assert_eq!(c.title.as_deref(), Some("M. Vale"));
        let names = &c.props["name"];
        assert_eq!(names.iter().map(|v| v.value.as_str().unwrap()).collect::<Vec<_>>(), ["M. Vale", "MARCUS VALE!"]);
        assert_eq!(names[1].provenance.iter().map(|p| p.fragment.as_str()).collect::<Vec<_>>(), ["m", "a"]);
        let p = &names[1].provenance[1];
        assert_eq!((p.source.as_str(), p.pointer.as_str(), p.field.as_str(), p.excerpt.as_str()), ("second", "at a", "f_name", ""));
        assert_eq!(c.conflicts, ["name"]);
    }

    fn cluster(id: &str, otype: &str, props: &[(&str, &[&str])]) -> Cluster {
        let props = props
            .iter()
            .map(|(name, vals)| {
                let vals = vals
                    .iter()
                    .enumerate()
                    .map(|(i, v)| PropValue {
                        value: Value::String(v.to_string()),
                        provenance: vec![Provenance { source: "s".into(), pointer: format!("line {i}"), field: name.to_string(), fragment: format!("{id}/{name}/{i}"), excerpt: String::new() }],
                    })
                    .collect();
                (name.to_string(), vals)
            })
            .collect();
        Cluster { id: id.into(), otype: otype.into(), fragments: vec![id.into()], sources: vec!["s".into()], props, title: None, conflicts: vec![] }
    }

    fn resolution(clusters: Vec<Cluster>) -> Resolution {
        Resolution { clusters: clusters.into_iter().map(|c| (c.id.clone(), c)).collect(), cluster_of: IndexMap::new() }
    }

    fn types(properties: &[&str], keys: &[&str]) -> IndexMap<String, ObjectType> {
        let fuzzy = Some(Fuzzy { properties: properties.iter().map(|s| s.to_string()).collect(), threshold: 0.86 });
        let otype = ObjectType { title: crate::pack::Fields::One("name".into()), keys: keys.iter().map(|s| s.to_string()).collect(), multi: vec![], fuzzy };
        IndexMap::from([("T".to_string(), otype)])
    }

    fn propose(res: &Resolution, otypes: &IndexMap<String, ObjectType>) -> Vec<Proposal> {
        proposals(res, &IndexMap::new(), otypes, &HashSet::new())
    }

    #[test]
    fn pinned_pairs_score_at_the_threshold() {
        for (a, b) in [
            ("Marcus Vale", "Marcus Vayle"),
            ("KESTREL COMPONENTS LIMITED", "Kestrel Components Ltd."),
            ("ALDER & FINCH PLASTICS", "Alder & Finch Plastics Co."),
            ("Brightline Electronics", "Brightline Electronics"),
            ("Nova Goods", "Nova Goods Co"),
        ] {
            let score = similarity(&fuzzy_norm(a), &fuzzy_norm(b));
            assert!(score >= 0.86, "{a} / {b}: {score}");
        }
    }

    #[test]
    fn proposal_carries_both_sides_and_conflicts() {
        let res = resolution(vec![
            cluster("b", "T", &[("name", &["Marcus Vayle"]), ("badge", &["B-1"]), ("email", &["m@x.io"])]),
            cluster("a", "T", &[("name", &["Marcus Vale"]), ("badge", &["B-2"]), ("email", &["M@X.IO"])]),
            cluster("c", "T", &[("name", &["Someone Else Entirely"])]),
        ]);
        let got = propose(&res, &types(&["name"], &["badge", "email", "phone"]));
        assert_eq!(got.len(), 1);
        let p = &got[0];
        assert_eq!(p.id, "a/name/0|b/name/0");
        assert_eq!((p.a.cluster.as_str(), p.b.cluster.as_str()), ("a", "b"));
        assert_eq!(p.a.value, "Marcus Vale");
        assert_eq!(p.b.value, "Marcus Vayle");
        assert_eq!(p.a.source, "s");
        // Badges differ; emails share a norm; nobody has a phone.
        assert_eq!(p.conflict, vec!["badge"]);
    }

    #[test]
    fn proposals_take_the_best_pair_across_properties() {
        let res = resolution(vec![
            cluster("a", "T", &[("name", &["Nova Goods"]), ("alias", &["Northwind Trading"])]),
            cluster("b", "T", &[("alias", &["Nova Goods Co", "Nova Goods"])]),
        ]);
        let got = propose(&res, &types(&["name", "alias"], &[]));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].score, 1.0);
        assert_eq!((got[0].a.property.as_str(), got[0].b.property.as_str()), ("name", "alias"));
        assert_eq!(got[0].b.value, "Nova Goods");
    }

    #[test]
    fn equal_scores_resolve_to_the_first_pair_in_config_order() {
        let res = resolution(vec![cluster("a", "T", &[("name", &["Nova"]), ("alias", &["Nova"])]), cluster("b", "T", &[("name", &["Nova"]), ("alias", &["Nova"])])]);
        let got = propose(&res, &types(&["name", "alias"], &[]));
        assert_eq!((got[0].a.property.as_str(), got[0].b.property.as_str()), ("name", "name"));
    }

    #[test]
    fn identical_names_are_proposed_for_every_pair_and_a_cluster_never_with_itself() {
        let res = resolution(vec![cluster("a", "T", &[("name", &["Nova"]), ("alias", &["Nova"])]), cluster("b", "T", &[("name", &["NOVA"])]), cluster("c", "T", &[("name", &["nova!"])])]);
        let got = propose(&res, &types(&["name", "alias"], &[]));
        let pairs: Vec<(&str, &str)> = got.iter().map(|p| (p.a.cluster.as_str(), p.b.cluster.as_str())).collect();
        assert_eq!(pairs, vec![("a", "b"), ("a", "c"), ("b", "c")]);
    }

    #[test]
    fn rejected_ids_and_other_types_and_empty_values_are_skipped() {
        let res = resolution(vec![
            cluster("a", "T", &[("name", &["Marcus Vale"])]),
            cluster("b", "T", &[("name", &["Marcus Vale"])]),
            cluster("c", "U", &[("name", &["Marcus Vale"])]),
            cluster("d", "T", &[("name", &["???"])]),
            cluster("e", "T", &[("name", &["!!!"])]),
        ]);
        let otypes = types(&["name"], &[]);
        assert_eq!(propose(&res, &otypes).len(), 1);
        let rejected = HashSet::from(["a/name/0|b/name/0".to_string()]);
        assert!(proposals(&res, &IndexMap::new(), &otypes, &rejected).is_empty());
    }

    #[test]
    fn output_is_sorted_by_type_then_clusters() {
        let mut otypes = types(&["name"], &[]);
        otypes.insert_before(0, "Z".to_string(), otypes["T"].clone());
        let res = resolution(vec![
            cluster("z1", "Z", &[("name", &["Nova"])]),
            cluster("z0", "Z", &[("name", &["Nova"])]),
            cluster("t3", "T", &[("name", &["Vale"])]),
            cluster("t1", "T", &[("name", &["Vale"])]),
            cluster("t2", "T", &[("name", &["Vale"])]),
        ]);
        let got: Vec<(String, String, String)> = propose(&res, &otypes).into_iter().map(|p| (p.r#type, p.a.cluster, p.b.cluster)).collect();
        let want: Vec<(String, String, String)> = [("T", "t1", "t2"), ("T", "t1", "t3"), ("T", "t2", "t3"), ("Z", "z0", "z1")].iter().map(|(t, a, b)| (t.to_string(), a.to_string(), b.to_string())).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn a_common_token_does_not_hide_a_pair_with_rarer_grams() {
        // Every name shares "ltd", a block far wider than the window.
        let mut clusters: Vec<Cluster> = (0..4 * BLOCK_WINDOW).map(|i| cluster(&format!("c{i:05}"), "T", &[("name", &[&format!("{:x} Ltd", i * 7919 + 1_000_000)])])).collect();
        clusters.push(cluster("x1", "T", &[("name", &["Quillfeather Bindery Ltd"])]));
        clusters.push(cluster("x2", "T", &[("name", &["Quillfeather Bindry Ltd"])]));
        let got = propose(&resolution(clusters), &types(&["name"], &[]));
        assert!(got.iter().any(|p| p.a.cluster == "x1" && p.b.cluster == "x2"), "{}", got.len());
    }

    #[test]
    fn neighbours_in_an_oversized_block_are_still_compared() {
        // Only grams shared with thousands of others; order in the block finds them.
        let clusters: Vec<Cluster> = (0..1000).map(|i| cluster(&format!("c{i:05}"), "T", &[("name", &[&format!("Seller {i:05}")])])).collect();
        let got = propose(&resolution(clusters), &types(&["name"], &[]));
        assert!(got.iter().any(|p| p.a.cluster == "c00500" && p.b.cluster == "c00501"), "{}", got.len());
    }

    #[test]
    fn anchor_finds_the_first_cluster_by_norm() {
        let res = resolution(vec![
            cluster("u", "Other", &[("name", &["Marcus Vale"])]),
            cluster("b", "T", &[("name", &["Someone", "Marcus Vale!"])]),
            cluster("a", "T", &[("name", &["marcus vale"])]),
        ]);
        assert_eq!(anchor(&res, "T", "name", "MARCUS VALE").map(|c| c.id.as_str()), Some("b"));
        assert_eq!(anchor(&res, "Other", "name", "marcus vale").map(|c| c.id.as_str()), Some("u"));
        assert!(anchor(&res, "T", "email", "marcus vale").is_none());
        assert!(anchor(&res, "T", "name", "nobody").is_none());
    }

    /// Timing and recall, not an assertion: `cargo test --release ... -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn proposals_scale_near_linearly() {
        let syllables = ["al", "der", "kes", "trel", "nor", "wind", "mar", "cus", "va", "le", "bright", "line", "no", "goods", "fin", "ch", "plas", "tic", "son", "ber", "ga", "ton", "ri", "ven"];
        let suffixes = ["Ltd", "Limited", "Co", "Inc", "Trading", "Holdings"];
        let word = |h: u64| -> String { (0..2 + (h % 3)).map(|k| syllables[((h >> (8 * k + 4)) % syllables.len() as u64) as usize]).collect() };
        for n in [10_000usize, 100_000, 1_000_000] {
            let mut clusters = Vec::with_capacity(n);
            let mut planted = HashSet::new();
            for i in 0..n {
                let h = (i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                let g = h.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
                let name = format!("{} {} {}", word(h), word(g), suffixes[(h >> 58) as usize % suffixes.len()]);
                clusters.push(cluster(&format!("c{i:07}"), "T", &[("name", &[&name])]));
                if i % (n / 1000) == 0 {
                    // A near duplicate: one byte changed somewhere in the name.
                    let mut bytes = name.into_bytes();
                    let at = (g >> 32) as usize % bytes.len();
                    bytes[at] = if bytes[at] == b'x' { b'y' } else { b'x' };
                    clusters.push(cluster(&format!("d{i:07}"), "T", &[("name", &[&String::from_utf8(bytes).unwrap()])]));
                    planted.insert((format!("c{i:07}"), format!("d{i:07}")));
                }
            }
            // Only pairs that actually reach the threshold count towards recall.
            let names: HashMap<&str, String> = clusters.iter().map(|c| (c.id.as_str(), fuzzy_norm(c.props["name"][0].value.as_str().unwrap()))).collect();
            let reachable: HashSet<(String, String)> = planted.into_iter().filter(|(c, d)| similarity(&names[c.as_str()], &names[d.as_str()]) >= 0.86).collect();
            let want = reachable.len();
            drop(names);
            let res = resolution(clusters);
            let otypes = types(&["name"], &[]);
            let start = std::time::Instant::now();
            let got = propose(&res, &otypes);
            let found = got.iter().filter(|p| reachable.contains(&(p.a.cluster.clone(), p.b.cluster.clone()))).count();
            println!("{n} clusters: {:?}, {} proposals, planted pairs found {found}/{want}", start.elapsed(), got.len());
        }
    }
}
