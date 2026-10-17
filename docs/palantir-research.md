# Palantir — research notes

What Palantir's platforms actually do, and which ideas this demo borrows.
Compiled October 2026 from Palantir's public documentation and product pages
(sources at the bottom). This is a learning summary, not an endorsement or a
complete feature list.

## The products

| Product | Audience | One-line summary |
|---|---|---|
| **Gotham** | Defense, intelligence, government | Investigation and operations: link analysis, geospatial, object analysis at scale |
| **Foundry** | Commercial | Data integration + the Ontology + operational apps |
| **AIP** | Both | The LLM layer: agents and LLM functions scoped to the Ontology |
| **Apollo** | Both | Continuous deployment of the above, cloud to edge |

Gotham and Foundry increasingly share one engine with different front doors.
Both orbit the same concept: **the Ontology**.

## The Ontology

Foundry's docs describe it as a "digital twin of the organization": a semantic
layer over datasets, virtual tables and models. The docs' own analogy:

| Data layer | Object layer (Ontology) |
|---|---|
| Dataset | Object type |
| Row | Object |
| Column | Property |
| Join | Link |
| — | Action type |

It has two halves:

- **Semantic** — object types, properties, link types, shared properties, and
  *interfaces* (polymorphism: several object types sharing a shape).
- **Kinetic** — **action types** (a single transaction that edits properties and
  links, with side effects, able to write back to external systems) and
  **functions** (code that takes objects / object sets in and returns objects or
  edits out).

Permissioning lives here too: Ontology roles on object, link and action types.

## The eight layers

### 1. Connect
Data Connection syncs from source systems. **Native Federation** backs object
types with data that stays in the external system, so nothing is copied.
Supports batch, micro-batch and streaming; structured, unstructured, IoT and
geospatial data.

### 2. Shape — Pipeline Builder
Foundry's primary integration app. Point-and-click; you define the target
schema (object types and properties) and describe how inputs map to it, and
the backend writes the transform code. It includes an LLM node so
classification, extraction, summarization and translation can be pipeline
steps.

### 3. Resolve identity — Entity Resolution
Matches records across disconnected systems that describe the same real-world
thing. Palantir describes hashing plus ML models plus fuzzy matching, with
tooling for human validation.

The detail worth stealing is from Gotham's object-resolution API. Resolving
objects merges them, but **each constituent keeps its own history, so the
merge can be undone later**, and properties written to the merged object can
be partitioned back out to the constituents. Identity is treated as a
reversible decision, not a destructive dedupe.

### 4. Explore
- **Object Explorer** — charts *are* the filter UI (histogram, listogram, pie,
  grid plot). You can filter on properties of *linked* object types, pivot the
  whole exploration to a linked type, undo/redo, and save layouts.
- **Vertex / Quiver graphs** — **Search Around**: multi-step, filtered traversal
  across object types (start from a set, follow a link, filter, follow another
  link, and the filters constrain the starting set). Histogram filters applied
  to the graph, dynamic node/edge styling, group-by-property layout, time
  windows, and a dependency view that narrows to one node's upstream and
  downstream.
- **Gotham Browser and Dossier** — view and edit an object's properties, add
  notes, read its change history; dossiers embed graphs and maps, and
  "mentioning" an object in a dossier links it.

### 5. Simulate — Scenarios
Vertex and Workshop support scenarios: branch the modeled world and ask
"what if?" under different conditions or decisions.

### 6. Act — Actions
Actions edit objects and links and can orchestrate external systems. When a
datasource update and a user edit disagree about the same object, a
per-datasource **conflict resolution strategy** decides (apply user edits, or
apply the most recent value).

### 7. Build apps — Workshop and OSDK
Workshop builds Ontology-aware apps from no-code to custom-code widgets. The
Ontology SDK lets external apps read objects and write back through actions.

### 8. Govern
- **Data Lineage** — interactive graph of how data flows; color nodes by health,
  staleness, or *a specific user's permissions* ("view as").
- **Data Health** — job, build, freshness, schema and content checks, alerting
  to PagerDuty, Slack, or REST endpoints.
- **Dataset transactions** — "Git for data": branch, snapshot, roll back.
- **Purpose-based access control** and audit.

### Plus: AIP and peering
- **AIP Logic** — no-code LLM functions over the Ontology; outputs can be applied
  automatically or **staged for human review**.
- **AIP Analyst** — natural-language question → an agent searches the Ontology,
  builds object sets, transforms data, and produces Vega charts and maps.
- **Peer Manager** — peer object and link types between enrollments (including
  cloud to edge), translating object IDs across the connection.

## The five ideas that make it feel like Palantir

1. **Fusion is the product.** The charts are not the point; the moment many
   differently-shaped sources become one object model is.
2. **Provenance at the property-value level.** Every value traces back to a
   source system, not just "this dataset came from there."
3. **Reversible entity resolution.** Identity is a hypothesis; un-merging is a
   first-class operation.
4. **Traversal as a query.** Search Around makes "who touched this, and what did
   they touch" a click.
5. **Kinetic, with a human gate.** Decisions write back out; AI produces
   proposals and humans commit them.

## How ELESSAR maps onto these

| Palantir concept | ELESSAR feature |
|---|---|
| Data Connection / federation | Six mock services over real HTTP, each with its own wire format; `transport: "http"` can point a source at a real URL |
| Pipeline Builder | `mapping.json`: declarative field → property mappings with transforms |
| Ontology object types | `mapping.json` `object_types`, with keys, title and fuzzy-match config |
| Entity Resolution | Exact-key auto-merge + fuzzy proposals you commit or reject; merges are reversible |
| Property-level provenance | Every property carries source, raw pointer (line / XPath / row) and the raw excerpt |
| Functions / logic | `signals.json`: a fixed set of rule kinds parameterized in data |
| Object views (Browser) | The side panel: properties with source chips, hits, belief derivation |
| Actions + audit | Judge, merge, crown — each mutates session state and appends to an audit log |
| AIP "propose, human commits" | Fuzzy merges are proposals; crowning a hypothesis is the user's call, checked against the posterior |
| Scenarios / what-if | Discounting or dismissing a signal re-runs inference live |

Not borrowed: geospatial, branching/versioning, permissions, streaming, LLMs.

## Prior art for the Bayesian board

- Champion & Elkan, *Visualizing the Consequences of Evidence in Bayesian
  Networks* — inference diffs and relevance filtering for large networks.
- shinyBN, bnviewer (R) — interactive BN viewers built on vis.js.
- jsbayes-viz, bayes.js — small JavaScript BN libraries; neither is maintained
  enough to depend on. ELESSAR implements its own noisy-OR propagation.

## Sources

- Platform overview — https://www.palantir.com/docs/foundry/platform-overview/overview/index.html
- Ontology core concepts — https://palantir.com/docs/foundry/ontology/core-concepts
- Ontology overview — http://palantir.com/docs/foundry/ontology/overview
- Foundry Ontology product page — https://palantir.com/platforms/foundry/foundry-ontology
- Action types — https://palantir.com/docs/foundry/action-types/overview
- Conflict resolution for actions — https://palantir.com/docs/foundry/action-types/getting-started
- Pipeline Builder — https://palantir.com/docs/foundry/pipeline-builder/overview
- Entity Resolution — https://www.palantir.com/foundry-entity-resolution/
- Gotham object resolution — https://www.palantir.com/docs/gotham/api/revdb-resources/resolution/resolution-basics
- Object Explorer charts — https://palantir.com/docs/foundry/object-explorer/explore-charts
- Vertex: explore object relationships — https://www.palantir.com/docs/foundry/vertex/explore-object-relationships
- Vertex: display options — https://www.palantir.com/docs/foundry/vertex/graphs-display-options
- Vertex scenarios — https://palantir.com/docs/foundry/vertex/scenarios-overview
- Quiver graph organization — https://www.palantir.com/docs/foundry/quiver/analysis-graph-organization
- Data Lineage — https://www.palantir.com/docs/foundry/data-lineage/overview
- Data Health — https://palantir.com/docs/foundry/observability/data-health
- AIP Logic — https://palantir.com/docs/foundry/logic/overview
- AIP Analyst — https://palantir.com/docs/foundry/aip-analyst/overview
- Ontology peering — https://palantir.com/docs/foundry/peer-manager/ontology-peering-overview
- Gotham service definition (UK G-Cloud) — https://assets.applytosupply.digitalmarketplace.service.gov.uk/g-cloud-14/documents/92736/801146272055049-service-definition-document-2024-05-02-1537.pdf
- Purpose-based access controls — https://blog.palantir.com/purpose-based-access-controls-at-palantir-f419faa400b3
- Champion & Elkan — https://arxiv.org/pdf/1707.00791
