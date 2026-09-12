# M3 — Resolver Shadow Fix, Next.js Priors, Stage-8 Flows

Date: 2026-09-12
Status: implemented. Exit criteria 1–4 met; criterion 5 is this section's
record. The live-agent runs stay credit-gated.
Implements: `code_arch_architecture.md` stage 8 (Flows) and the "Next.js first"
half of M3. Django waits on the M4 Python resolver — recorded here, not
silently dropped. The M2-era note in `graph.rs` ("embedding similarity arrives
at M3") is superseded: embeddings stay out; the measured Tier-2 gap turned out
to be a resolver bug, not a missing signal.
Evidence: commerce profiling, 2026-09-12 (9/9 route files with ~zero resolved
out-edges; see Problem).

## Problem

M3's exit is "measurable improvement on Tier 2 repositories in the M1
benchmark". The available Tier-2 checkout is commerce (Next.js, 76%
resolution). Profiling it showed flows cannot be the first move:

```text
route file                        cluster   reachable files (depth 4)
app/[page]/layout.tsx             0         0
app/[page]/page.tsx               0         0
app/api/revalidate/route.ts       8         0
app/layout.tsx                    5         0
app/page.tsx                      5         0
app/product/[handle]/page.tsx     2         0
app/search/[collection]/page.tsx  0         0
app/search/layout.tsx             0         1
app/search/page.tsx               0         0
```

Commerce route files import bare specifiers (`components/grid`,
`lib/shopify`) via tsconfig `baseUrl: "."`. `resolve_one` handles baseUrl
(`resolve.rs:185`), but `resolve_all` never calls it for these:
`is_external_specifier` (`resolve.rs:237`) classifies any lowercase bare path
as a package first, so the imports are filed as externals and carry no edge.
Flows traversed on this graph would be empty lines on exactly the repo M3
must improve. The shadow fix is therefore phase 1, not a separate bugfix.

Entry points themselves work: `route_hints` fires app/pages routes plus
generic entries. What is missing is the rest of the App Router convention
(`loading`, `error`, `not-found`, …) and anything downstream of an entry.

## Goals

- Unshadow baseUrl/alias resolution from the package heuristic.
- Complete the Next.js App Router entry conventions (path-only, no parser).
- Stage 8: deterministic entry→import-chain flows in high-band domains,
  rendered inside the existing 4,000-token budget.
- Measure on the free deterministic suite; record win or loss.

## Non-goals

- Django priors or any Python parsing (M4).
- Call edges. `parse.rs` extracts import specifiers only; "import and call
  edges" in the stage-8 contract means import edges in this implementation.
- Cross-cluster flow traversal. Inter-cluster deps already render as
  "Depends on"; flows stay intra-cluster where the subsystem claim holds.
- Model-generated flows. Traversal output or absent — the contract's
  highest-cost-error rule stands.
- Precomputed transitive closure (rejected with the reverse index: hub files
  reach most of the repo).

## Design

### Phase 1 — shadow fix (`src/resolve.rs`)

For non-relative specifiers, try the alias/baseUrl lookup before the package
heuristic: hit means internal; miss falls through to the existing
`is_external_specifier` → external, else unresolved. Relative and `@`-scoped
specifiers already reach `resolve_one` and are untouched.

Accepted risk: an npm package name colliding with a repo file path now
resolves to the repo file. `node_modules` is excluded from the inventory, so
the repo file is the saner reading; documented at the call site.

### Phase 2 — Next.js priors (`src/profile.rs`)

`next_app_kind` gains the route-adjacent conventions as entries of their
route: `loading`, `error`, `not-found`, `opengraph-image`, `sitemap`,
`robots`, `manifest`, `favicon`, `icon`, `apple-icon`. Labels read
`loading /search`, `not-found /product/:handle`, etc. Route groups,
parallel slots, page/route/layout handling unchanged.

### Phase 3 — flows (`src/flows.rs`, stage 8)

One subprocess-free pure computation:

```text
In     CodeGraph (import out-edges), Partition, Importance, RouteHints, band per cluster
Out    Vec<Flow> per cluster, Flow { entry_label, entry_file, path: Vec<FileId> }
Fails  No entries, non-high band, or empty traversal → no flows. Never fabricated.
```

From each entry-point file in a high-band domain, BFS forward along
out-edges, depth capped at 4, members only. Nodes below an importance floor
(bottom quartile of cluster scores) are pruned unless they are the entry
itself. Terminal pure leaves (no out-edges within the cluster) collapse to
`→ … (+n leaves)`. Cap 3 flows per domain by entry importance; ties by path.

Render: a `Flows:` subsection per high-band domain with entries, after Key
files:

```md
Flows:

- `app/search/page.tsx` → `components/grid/index.tsx` → `lib/shopify/index.ts`
```

`index.json` clusters gain `flows` as arrays of rel-path arrays. The "no
execution flows" caveat line is replaced by a flows/no-flows statement in
both arms (flows present vs. none identified), keeping the honest boundary.

### Budget

The ladder gains a flows dimension: build with flows first; if over budget,
drop flows before shrinking file lists. Enhancement yields to core content.
Commerce (~2,000 tokens) should keep flows; hono (~3,400) exercises the drop
path or keeps them — measured, not assumed.

### CLI

No new flags. `--no-git` and `--codearch-dir` behavior unchanged.

## Measurement

All free and deterministic; live-agent runs stay credit-gated:

1. Resolution per checkout (commerce 76% → ?, hono 98% and typedi 100%
   regression check).
2. Flow coverage: entries with non-empty flows over total entries, per repo.
3. Byte-identical ×2 on all three checkouts; 4,000-token cap held.
4. M1 lexical-proxy harness (`python eval/run.py`) delta — the literal exit
   criterion suite.
5. Ceiling re-run (`check_import_index.py`), expected unchanged at 1.000.
6. Results into `code_arch_architecture.md` win or loss.

## Testing

Hermetic, as before:

- Resolver: baseUrl bare import resolves when the file exists; still
  external when it does not; real packages (`react`, `zod`) unaffected;
  alias shadowing unchanged; relative specs untouched.
- Priors: each new kind maps to the right label and route; non-route files
  unaffected; `M1 SYSTEM` prompt byte-identical (existing test).
- Flows: linear chain renders full path; depth cap truncates; importance
  floor prunes; leaves collapse; cycles terminate; cross-cluster edges stop
  the path; medium/low bands and entry-less domains emit nothing.
- Determinism: two consecutive runs byte-identical on all three checkouts.

## Exit criteria

1. `cargo test` (existing unmodified + new) and `python -m pytest eval` pass.
2. Commerce resolution improves with no hono/typedi regression; maps stay in
   budget and byte-identical.
3. Flows render on commerce with measured coverage; hono either keeps flows
   in budget or deterministically drops them.
4. M1 harness delta recorded; ceiling re-run recorded.
5. Results in `code_arch_architecture.md` win or loss; Django-to-M4 recorded.

## Risks

**The shadow fix moves clustering.** New edges change Leiden partitions and
churn-derived names; the M2 delta maps shift underneath. That is the point
(the old partitions were built on a censored graph), but `--no-git`-style
reversibility does not exist for this one — it is a correctness fix, not a
signal addition. Mitigation: resolution + ceiling + M1 numbers before/after,
all free.

**Flows add tokens without helping.** Ordered chains may duplicate what Key
files + Depends-on already say. The M1 harness delta is the check, not an
assumption; a zero/negative delta is recorded as such.

**The M1 lexical proxy may not see flows.** It ranks map lines by query-word
overlap; path chains are filename-dense but query-word-sparse. If the proxy
cannot use them, that bounds the proxy, not necessarily an agent — stated in
the results, not hidden.
