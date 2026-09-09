# Code Arch — Architecture

> Companion to `code_arch_core_idea.md`. That document states what the tool is for.
> This one states how it is built, in what order, and what it does when it cannot be sure.

---

## Design Principles

Five constraints drive every decision below. When a choice is ambiguous, resolve it against these.

```text
1. Structure is extracted, never invented.
   The graph decides what a subsystem is. The model only names it.

2. Uncertainty is reported, not smoothed over.
   A confidently wrong map is worse than an absent one, because the agent trusts it.

3. Determinism first, model last.
   Anything computable by a parser, a traversal, or git is not a model call.

4. The output has a token budget, and the budget is enforced.
   A map that costs what the repository costs has no reason to exist.

5. The reader is an agent, not a person.
   Optimize for uniformity, density, and machine routing — not readability.
```

---

## Resolved Design Questions

The core idea document left two questions open. Both are settled here.

### Decision 1 — The generative model is code-pretrained, and prose quality is removed as a variable

**The question as posed:** code-specialized models are tuned to continue code, not to describe it in English. A general model might describe better even if it recognizes identifiers worse.

**Resolution:** use `Qwen2.5-Coder-1.5B-Instruct`, and restructure the task so prose quality stops being model-dependent.

Three things dissolve the concern:

**The stated failure mode belongs to base models.** "Continues code instead of describing it" is a property of a base checkpoint. The Instruct variant is post-trained on instruction-following data that includes code explanation and QA. It writes acceptable English about code because it was trained to.

**The output is not prose.** Under GBNF constraint the model emits:

```json
{ "name": "Session Lifecycle", "summary": "Issues, validates, and expires user sessions for the HTTP layer." }
```

That is a constrained naming task — three to six words plus one capped sentence — not open-ended generation. Fluency has almost no room to vary.

**The actual failure modes at this scale are not prose failures.** They are:

```text
Generic labels        "Core Utilities", "Application Services"
Sibling collision     three adjacent clusters all named "Services"
Ungrounded nouns      a capability named in the summary that appears
                      in no identifier, path, or dependency in the input
```

All three are grounding and discrimination problems. Code pretraining directly helps both — the model's job is recognizing that `authenticateUser`, `validateResetToken`, `UserRepository`, `middleware.ts` cohere as a concept. General pretraining does not help there and buys only sentence smoothness, which the consumer does not read for.

**The escape hatch is a smaller task, not a bigger model.** If summary quality proves weak in evaluation, the response is not to swap in a 4B general model. It is to template the sentence and let the model fill slots:

```text
{Name} handles {gerund phrase} for {domain},
entered through {top 2 entry points}.
```

A slot-filled sentence is grounded by construction, uniform across the document, and cheaper to generate. For an agent reader this is strictly better than free-form text.

**How it gets verified:** the ~20-cluster hand-labeled evaluation set, scored on three mechanical metrics — name specificity (does the name distinguish this cluster from its siblings?), groundedness (is every content noun traceable to an input token?), and collision rate across siblings. Not fluency ratings. Swapping a candidate GGUF and re-running is a ten-minute test.

**Prediction on record, to be falsified by that test:** the code model wins on groundedness and identifier recognition; a general 2B writes marginally smoother sentences that no consumer benefits from.

### Decision 2 — Large repositories split by clustering resolution, not by a bespoke heuristic

**The question as posed:** for very large repositories, one file becomes a top-level map plus per-domain context files. How is that split decided?

**Resolution:** do not design a splitting rule. Leiden already takes a resolution parameter, so run the clustering twice and read the hierarchy off it.

```text
Leiden @ coarse resolution  →  8–12 top-level domains   →  CODEBASE.md
Leiden @ fine resolution    →  40–120 subsystems        →  .codearch/domains/*.md
                                (each nested under the coarse cluster
                                 containing the majority of its files)
```

The hierarchy is therefore a property of the same graph that produced the flat map, using the same algorithm, with the same confidence machinery. No second notion of "grouping" is introduced.

**Trigger.** Splitting is a budget outcome, not a repository-size rule. Render the flat map first; if it exceeds the root budget, split. Roughly, repositories under ~300 source files never split and very large ones always do, but the count is a consequence, not the condition.

**Layout.**

```text
CODEBASE.md                     committed, always the entry point, hard cap 4,000 tokens
.codearch/
    domains/
        authentication.md       loaded on demand
        billing.md
        ...
    index.json                  machine-readable graph, clusters, confidence, hashes
    cache/                      parse and label caches, gitignored
```

**Contract between the two levels.** This is the part that has to hold or the split fails:

```text
CODEBASE.md is sufficient to answer:  "which domain is this task in,
                                       and which file do I read next?"

domains/<slug>.md is required for:    execution flows, the ranked file list,
                                       subsystem-internal relationships
```

The root file therefore always ends with an explicit routing table, so the agent picks one child file without reading any of them:

```text
## Routing

subscription, checkout, invoice, stripe, plan   → .codearch/domains/billing.md
login, session, token, password, oauth          → .codearch/domains/authentication.md
migration, schema, query, repository            → .codearch/domains/persistence.md
```

Keywords are extracted deterministically — identifier frequency within the cluster, minus terms appearing in more than one domain, so routing terms are discriminative by construction.

**Budget allocation across children.** Each domain file gets a share proportional to `cluster_size × log(1 + churn)`, floored at 300 tokens and capped at 1,200. Hot subsystems earn more space; a large dormant vendored region does not.

---

## Pipeline

Ten stages. Exactly one calls a model.

```text
0  Inventory        walk, classify, exclude
1  Profile          manifests → ecosystem and framework priors
2  Parse            tree-sitter → symbols, references, exports, routes
3  Resolve          reference strings → concrete file targets
4  Git signals      co-change, churn, recency
5  Graph            weighted multi-signal graph
6  Cluster          Leiden, two resolutions, stability-checked
7  Rank             importance scoring and budget selection
8  Flows            entry-point traversal
9  Label            ← the only LLM stage
10 Render           budget-aware assembly
```

Stages 0–8 and 10 are deterministic. Re-running them on an unchanged repository produces a byte-identical result. Stage 9 is cached by content hash, so it is effectively deterministic too.

---

## Stage Contracts

Every stage has one job, a typed input, a typed output, and a defined behavior on failure. Failure is local: a stage that cannot do its work degrades its own output and lowers confidence downstream — it does not abort the run.

### 0 — Inventory

```text
Job       Enumerate candidate source files and classify what to exclude.
In        Repository root path, optional .codearchignore
Out       FileInventory { path, language, bytes, loc, class }
          class ∈ source | test | config | generated | vendored | asset | binary
Fails     Unreadable path → skipped, recorded in diagnostics.
```

Exclusion is layered and conservative: `.gitignore`, then known vendor and build directories (`node_modules`, `vendor`, `dist`, `target`, `.next`), then generated-file markers (`@generated`, `Code generated by`, lockfiles, `*.pb.go`, `*_pb2.py`), then a minified/long-line heuristic. Excluded files are counted and reported, never silently dropped — the header line "excluded 812 generated files" is itself useful context.

### 1 — Profile

```text
Job       Identify ecosystems and frameworks before any parsing.
In        FileInventory
Out       EcosystemProfile { ecosystems[], frameworks[], entry_point_rules[],
                             path_mappings, confidence }
Fails     No recognized manifest → empty profile, tier assumed lowest, run continues.
```

Reads `package.json`, `pyproject.toml`, `go.mod`, `Cargo.toml`, `pom.xml`, `Gemfile`, `composer.json`. This runs first because it changes how later stages behave: a detected Next.js project means `app/**/page.tsx` are routes and `middleware.ts` is authentication-adjacent, both of which are facts no parser recovers. Framework priors are hardcoded knowledge and that is the intended design — convention frameworks are simultaneously where import graphs fail worst and where hardcoded priors work best.

### 2 — Parse

```text
Job       Extract syntax-level facts per file.
In        FileInventory, EcosystemProfile
Out       SymbolTable   { file, symbol, kind, span, exported }
          RawEdges      { from_file, raw_target, edge_kind }
          RouteHints    { file, method, path_pattern }
Fails     Parse error → tree-sitter returns a partial tree; usable nodes kept,
          file flagged partial. Never fatal.
```

Tree-sitter queries per language, kept as declarative `.scm` files so adding a language is data rather than code. This stage reports that a symbol was referenced — never which definition it resolves to. That is stage 3's job, and conflating them is the single most common way this kind of tool becomes quietly wrong.

### 3 — Resolve

```text
Job       Turn raw reference strings into concrete file targets.
In        RawEdges, EcosystemProfile, FileInventory
Out       ResolvedEdges { from_file, to_file, kind, confidence }
          UnresolvedRate per file and per directory
Fails     Unresolvable reference → dropped from the graph, counted.
          The unresolved rate is the primary confidence input for the whole run.
```

This is where the ecosystem-specific engineering lives: `tsconfig.json` path aliases and `baseUrl`, `package.json` exports maps, Python package semantics and relative import depth, Go module paths, Java classpath and package roots. It does not generalize, and attempting to make it generalize is how a tool ends up supporting many ecosystems shallowly and none of them correctly.

A directory whose unresolved rate exceeds ~40% is a Tier 2/3 signal: the architecture is not in the imports there, and the graph must lean on git and embeddings for that region.

### 4 — Git signals

```text
Job       Derive relationship evidence that requires no parser.
In        Repository git history (bounded: last ~2,000 commits or 24 months)
Out       CoChange { file_a, file_b, weight }
          Churn { file, commits, last_touched, author_count }
Fails     Not a git repository, or shallow clone → stage emits empty output,
          graph proceeds on imports alone, confidence lowered globally.
```

Files repeatedly modified in the same commit tend to belong to the same subsystem. This is behaviorally identical across languages and captures exactly what static analysis misses: dependency-injection wiring, event producers and consumers, config-and-implementation pairs, annotation-driven registration.

Two corrections matter. Large commits are noise, so weight each pair by `1 / (files_in_commit - 1)` — a two-file commit is strong evidence, a 300-file refactor is nearly none. And recent commits count more, via exponential decay on commit age.

Churn additionally feeds importance ranking and budget allocation: a file nobody has touched in three years is rarely where the next task lands.

### 5 — Graph

```text
Job       Fuse all signals into one weighted graph over files.
In        ResolvedEdges, CoChange, FileInventory, optional Embeddings
Out       CodeGraph — nodes = files, edges = weighted sum by signal
Fails     Cannot fail. Degrades to a directory-adjacency graph in the worst case.
```

```text
import / call edge         1.00    strong, directional, ground truth
co-change (normalized)     0.55    language-agnostic, noisy alone
directory adjacency        0.25    weak prior, prevents fragmentation
embedding similarity       0.35    only for files with unresolved-heavy neighborhoods
```

Embeddings are a targeted fallback, not a default signal. They are computed only for files in regions where import resolution failed — the Tier 2 and Tier 3 case — which keeps the second model component cheap and stops semantic similarity from overriding actual dependencies where those exist.

**Signal agreement is recorded here**, not just signal sum. Where imports and co-change agree, confidence rises. Where co-change insists two files belong together and the import graph says nothing, that is either hidden coupling (valuable) or noise (dangerous) — flag it, weight it, and let the confidence model treat that cluster as less certain.

### 6 — Cluster

```text
Job       Partition the graph into subsystems at two resolutions.
In        CodeGraph
Out       ClusterTree { coarse[], fine[], membership, stability }
Fails     Graph too sparse to cluster meaningfully → fall back to directory
          structure as the partition, mark every cluster low-confidence.
```

Leiden rather than Louvain, because Louvain can produce internally disconnected communities and here that means a "subsystem" whose members do not actually relate.

**Stability check.** Run the clustering across several seeds and slightly perturbed resolutions. Files that land in the same cluster every time are stable; files that migrate between runs are boundary cases. Per-cluster stability — the fraction of members that never moved — is a direct, cheap, honest confidence input. A cluster at 0.95 stability is a real seam in the codebase. One at 0.5 is an artifact of the algorithm and must be presented as such.

Directory structure is applied here as a soft correction, not a hard constraint: it nudges the partition toward folder boundaries where the graph is ambivalent, and is overruled where the graph is clear.

### 7 — Rank

```text
Job       Score importance so the budget spends itself on what matters.
In        CodeGraph, Churn, RouteHints, ClusterTree
Out       Importance { file, score } per cluster, ranked
Fails     Cannot fail.
```

```text
score = 0.35 · pagerank(graph)          structural centrality
      + 0.25 · entry_point_bonus        routes, CLI mains, public exports, handlers
      + 0.20 · normalized_churn         where work actually happens
      + 0.20 · fan_in                   how many things break if this changes
```

An agent asking "where do I look" is asking about high-fan-in, high-churn, entry-point-adjacent files. Line count is deliberately absent — a 2,000-line generated constants file is not important.

### 8 — Flows

```text
Job       Produce readable execution paths through each subsystem.
In        CodeGraph, RouteHints, EcosystemProfile, Importance
Out       Flow { entry, ordered_nodes[], terminal }
Fails     No identifiable entry points → subsystem gets a ranked file list
          and no flow section. Never a fabricated flow.
```

Entry points come from framework priors — HTTP routes, CLI `main`, job schedulers, event handlers, exported public API. From each, breadth-first traversal forward along import and call edges, depth capped at 4, pruning nodes below an importance floor and collapsing pure leaf utilities.

This is the stage most tempting to hand to the model, and it must not be. A hallucinated flow is the highest-cost error the tool can make: it reads as precise, an agent will act on it, and it is wrong. Flows are traversals or they are absent.

### 9 — Label

```text
Job       Name and describe clusters the graph already found.
In        ClusterSummary — deterministic, compact, no raw source
Out       { name, summary } per cluster, GBNF-constrained
Fails     Generation failure or timeout → fall back to derived name
          (dominant directory + dominant identifier stem) and templated summary.
          A run never blocks on the model.
```

The model receives a structured summary, never code:

```text
Directories:     src/auth/, src/middleware.ts
Top symbols:     authenticateUser, createSession, validateResetToken, revokeSession
Entry points:    POST /api/login, POST /api/logout, middleware.ts
External deps:   jsonwebtoken, argon2
Depends on:      users, email
Depended on by:  billing, admin
Sibling names:   Billing, User Management, Persistence
```

Four implementation requirements:

```text
GBNF grammar        valid JSON is structurally impossible to violate,
                    leaving only quality to tune
Few-shot exemplars  worth more than doubling parameters at this scale
~40 token cap       output tokens are the CPU bottleneck
Sibling names in    prevents three adjacent clusters all named "Services"
context
```

Caching is keyed on a hash of the ClusterSummary. Unchanged clusters are never re-labeled, which is what makes incremental runs nearly free — this stage is the only cost of any consequence.

### 10 — Render

```text
Job       Assemble output within a hard token budget.
In        everything above
Out       CODEBASE.md, .codearch/domains/*.md, .codearch/index.json
Fails     Cannot fail. Budget overflow truncates by ascending importance.
```

Budget is enforced by measured tokens, not estimated characters, using the same tokenizer family the consuming agents use. The root file is rendered first; if it exceeds 4,000 tokens, Decision 2's split activates and rendering restarts in hierarchical mode.

---

## Confidence Model

Principle 2 requires that uncertainty be visible. Confidence is computed per cluster from three inputs already produced by the pipeline:

```text
resolution_rate     fraction of that cluster's references resolved to real files
stability           fraction of members that never migrated across seeds
agreement           correlation between import edges and co-change edges
                    within the cluster

confidence = 0.45·resolution_rate + 0.35·stability + 0.20·agreement
```

Three bands, each with a different rendering:

```text
≥ 0.75    high     full treatment — name, summary, flows, ranked files
0.45–0.75 medium   name, summary, ranked files, no flows;
                   marked "relationships partially inferred"
< 0.45    low      files listed, grouped by directory, no claimed relationships,
                   explicit line: "structure could not be reliably determined
                   for this region"
```

The low band is a feature. A tool that never emits it on a Tier 3 repository is not measuring anything.

Global confidence is reported in the header so an agent can calibrate how much to trust the file as a whole.

---

## Output Format

```md
# Codebase Map — <project name>

Generated by Code Arch · <commit sha> · confidence: high
1,284 source files · ~820,000 source tokens · this map: 3,900 tokens
Excluded: 812 generated, 340 vendored, 96 assets

## What This Is
<2–3 sentences: product, not implementation>

## Stack
<from manifests — deterministic, never model-generated>

## Domains
### Authentication · confidence high
<one-line summary>
Entry points: POST /api/login, middleware.ts
Key files:
  src/auth/session.ts        session lifetime, validation
  src/auth/login.ts          credential check, token issue
  src/auth/password_reset.ts reset token flow
Flows:
  POST /api/login → validateCredentials → UserRepository → createSession
Depends on: Users, Email · Depended on by: Billing, Admin

### <...>

## Task Navigation
<task phrasing → file set, derived from flows and importance>

## Routing            ← only present when split
<keywords → domain file>

## Not Analyzed
<excluded paths and low-confidence regions, stated plainly>
```

Two rules govern the format. Every factual claim traces to a deterministic stage — only the domain names and one-line summaries originate from the model. And "Not Analyzed" is mandatory, because the honest boundary of the map is part of the map.

---

## Incremental Updates

A second run on a lightly changed repository must be fast, or the tool will not be re-run and the map will rot.

```text
.codearch/index.json stores:
    last commit sha
    per-file content hash
    cluster membership and per-cluster summary hash
    model id and prompt version
```

```text
git diff since stored sha
    ↓
re-parse only changed files
    ↓
re-resolve only edges touching changed files
    ↓
re-cluster only if graph edit distance exceeds threshold
        (small changes preserve the existing partition)
    ↓
re-label only clusters whose ClusterSummary hash changed
    ↓
re-render always (cheap)
```

Typical result: a run touching a dozen files re-labels zero or one cluster. `--force` rebuilds everything. Changing the model or the prompt version invalidates all label caches automatically.

---

## Implementation Stack

**Language: Rust.** Tree-sitter is natively Rust, so the parsing layer is first-class rather than an FFI binding. A single static binary matches the intended UX — `codearch .` with no runtime, no virtualenv, no version conflict with the user's own toolchain. And stages 0–8 are graph and traversal work over thousands of files, where a scripting language becomes the bottleneck on exactly the large repositories that motivate the tool.

The cost is that per-language resolvers must be written in Rust rather than borrowed from each ecosystem's own tooling. That is real, and it is the argument for supporting few ecosystems properly.

```text
Parsing        tree-sitter (Rust), grammars vendored, queries as .scm data
Graph          petgraph
Clustering     Leiden — implemented directly, roughly 300 lines
Inference      llama.cpp via llama-cpp-2, GGUF, GBNF grammar constraint
Embeddings     same runtime, small code embedding model, loaded only when needed
Tokenizing     tokenizers crate, for budget enforcement
Git            gix (pure Rust, no libgit2 build dependency)
```

Model files are downloaded on first run into a platform cache directory, not bundled in the binary. `--model <path>` overrides, per the core idea's swappability requirement. After first run the tool is fully offline.

```text
codearch/
    crates/
        core/         pipeline orchestration, stage contracts, config
        inventory/    stages 0–1
        parse/        stage 2, grammars and queries
        resolve/      stage 3 — one module per ecosystem
        signals/      stage 4
        graph/        stages 5–8
        label/        stage 9, llama.cpp integration, prompts, GBNF
        render/       stage 10, budget enforcement
        cli/          binary
    grammars/
    prompts/          versioned, hashed into the label cache key
    eval/             harness, fixtures, hand-labeled cluster set
```

---

## Build Order

Each milestone ends at something runnable and measurable.

```text
M0   TypeScript / JavaScript only, Tier 1, end to end.
     Stages 0–3, 5–7, 9–10. No git signals, no flows, no split.
     Exit: codearch . on a mid-size Next.js repo produces a useful map.

M1   Evaluation harness — before adding any breadth.
     Downstream task benchmark (tokens, files opened, searches, correctness)
     plus the ~20-cluster labeling set.
     Exit: a number exists for "does the map help".
     This is deliberately second. Everything after it is guesswork otherwise.

M2   Git co-change and the confidence model. Stage 4 and the banding.
     Exit: the tool emits low-confidence regions on a Tier 3 repository
     instead of guessing.

M3   Framework priors and flows. Stage 8, Next.js and Django first.
     Exit: measurable improvement on Tier 2 repositories in the M1 benchmark.

M4   Python ecosystem resolver.
     Exit: two ecosystems supported properly.

M5   Hierarchical split and incremental updates. Decision 2, plus caching.
     Exit: a 5,000-file repository produces a 4,000-token root map,
     and re-running after a small change takes seconds.

M6   Model evaluation sweep and the LoRA fine-tune path from the core idea.
     Exit: model choice made from the M1 numbers rather than from reasoning.
```

---

## Implementation Status — M0 complete

M0 is built and runs end to end: `codearch <path>` produces `CODEBASE.md` and
`.codearch/index.json`. 44 unit tests pass. Verified on two real repositories —
`honojs/hono` (377 files, ~686k estimated source tokens → a 3,126-token map at
98% import resolution, 4.1s) and `vercel/commerce` (64 files, 76% resolution).
Two consecutive runs produce byte-identical output.

Seven deviations from the design above, each deliberate:

```text
Single crate, lib + bin        Nine crates is ceremony at this size. The module
                               boundaries match the stage list, so splitting
                               later is mechanical.

Node-kind tables, not .scm     tree-sitter 0.27 returns a StreamingIterator from
                               `matches`, and a query naming a node kind absent
                               from a grammar version fails to compile outright.
                               A table keeps "adding a language is data, not
                               code" while degrading silently on grammar drift.

Own graph structures, no       Leiden and PageRank both wanted custom
petgraph                       adjacency anyway; the dependency bought nothing.

Louvain + connectivity         Full Leiden refinement is deferred. The
guarantee, not full Leiden     connectivity property — no internally
                               disconnected community, enforced at every level —
                               is the part the architecture actually chose
                               Leiden for, and it is implemented and tested.

Stage 9 ships DerivedLabeler   The declared fallback path: derived name plus
only                           templated summary, no model. llama.cpp slots in
                               behind the existing `Labeler` trait at M0.5.

Domain cap is soft             Reported, not forced. See below.

Balance criterion added        See below.
```

Two findings from running it on real code, both of which changed the design:

**Domain ordering is a first-class concern, not a rendering detail.** The first
hono map opened with four `benchmarks/` domains and buried the library. Ranking
was being applied *within* domains but never *across* them. Domains are now
ordered by summed member importance, and cluster cross-references are renumbered
to follow. Directory naming is likewise weighted by importance rather than file
count, so a domain centred on `src/context.ts` and `src/hono.ts` is not named
for a populous `src/utils`.

**Optimizing the domain count alone produces the opposite failure.** Driving the
resolution down until the count fit a cap of 12 handed back one domain holding
272 of 377 files — technically within the cap, and not a decomposition of
anything. Stage 6 now scans a fixed resolution ladder and picks the partition
that fits the cap *while leaving the largest domain smallest*, capped at 40% of
the repository. Hono then decomposes into its real subsystems — Jsx, Routers,
Ssg, Jwt, Streaming, Etag — at the cost of 16 domains rather than 12.

That cost is the right trade, so the cap became soft. Where the remaining groups
share no imports at all, merging them further would assert a relationship the
code does not have, and the tool says so instead:

```text
Note: 16 domains exceeds the cap of 12. The remaining groups share no imports,
so merging them further would assert a relationship the code does not have.
```

Not yet measured: whether any of this actually helps an agent. That is M1, and
until it exists every number above describes the tool rather than its value.

---

## Failure Modes

The ones that would actually sink the tool, and what is in place against each.

```text
Confidently wrong map          confidence banding, stability check,
                               low-confidence band that refuses to claim structure

Hallucinated flow              flows are graph traversals; the model never
                               produces one

Fabricated dependency          every relationship traces to a resolved edge
                               or a co-change pair; the model receives no
                               raw source and cannot introduce edges

Map rots after a week          incremental updates are cheap enough
                               to run on a hook or in CI

Output too large to be worth   hard token budget, measured not estimated,
it                             enforced at render with importance-ordered truncation

Generic useless labels         sibling names in prompt context, groundedness
                               metric in the eval set, template fallback

Silent Tier 2/3 degradation    unresolved rate is a first-class metric that
                               propagates into visible confidence
```

---

## Still Open

```text
Monorepos with several ecosystems in one tree — likely one profile and one
graph per workspace package, joined at the top level, but unverified.

Whether "Task Navigation" should be generated per repository or derived from
observed agent behavior over time. The latter is more useful and much harder.

Whether CODEBASE.md should be committed. Committing shares the map across a
team and across agents; it also produces diff noise on every structural change.
Current lean: commit the root map, gitignore .codearch/cache/, leave domain
files to the user's preference.

Cross-language edges — an API contract between a TypeScript frontend and a
Python backend is a real dependency that no single-language resolver sees.
Co-change catches some of it. Nothing catches the rest yet.
```

## Initial M1 proxy status — superseded by the real-agent run below

The evaluation harness lives in `codearch/eval/README.md`. It runs paired
with-map/without-map file-location tasks and records cl100k_base context volume,
distinct files opened, searches, exact-answer correctness, and full traces.
It includes 40 synthetic tasks across all three visibility patterns, 12 pinned
Hono tasks, a 20-case authored seed labeling set, and an external-agent protocol.
The seed set is not independently human-reviewed or extracted production clusters.

The fixed lexical navigation policy measured 32/40 -> 40/40 correctness on the
synthetic set, at 15,539 -> 40,594 context tokens (+161.24%). On Hono it measured
0/12 -> 2/12 correctness and 159,130 -> 86,570 tokens (-45.60%). The Hono accuracy
is too low to support an efficacy claim. These are retrieval-proxy numbers, not
LLM coding-agent results or provider-billed token measurements.

No M2+ breadth was added. The runnable numeric baseline exists, but the stronger
M1 efficacy gate remains open pending representative real-agent runs and
independent labeling review. Full reports are under `codearch/eval/results/`
and `codearch/eval/results-hono/`.


## M1 real-agent result — measured, negative on this navigation suite

The remaining M1 run is now complete: 24 source-inspected tasks from pinned Hono,
Next.js Commerce and TypeDI checkouts, two fresh trials per map/no-map condition
(96 episodes). Qwen3.5 Flash ran through OpenRouter/Alibaba at temperature zero,
with a fixed seed and reasoning disabled. All 204 calls completed without provider
errors. Both conditions received the complete source-file inventory and identical
tools; the mapped condition additionally received the entire generated map.

| Metric | Without map | With map |
|---|---:|---:|
| Exact file-set correctness | 46/48 | 45/48 |
| Delivered context tokens | 159,821 | 289,899 |
| Provider input + output tokens | 304,432 | 605,301 |
| Distinct files opened, summed over episodes | 51 | 53 |
| Searches | 2 | 2 |
| Provider-reported USD | 0.020140 | 0.039704 |

Under this protocol the map did not help: 98.83% more provider tokens, no search
reduction and one fewer correct answer. It added context in all three tiers.
This is not proof that maps never help: tasks primarily ask for one implementation
file, descriptive paths make the inventory baseline strong, and repeated trials
are correlated. Architectural questions and code-change tasks were not measured.
The map generator was not tuned after seeing these results.

The real labeling set now contains 20 extracted production clusters (top ten each
from Hono and Commerce). References were authored from structured source evidence
and independently reviewed by GPT-4.1 Mini without candidate outputs. They are
model-reviewed, not human-reviewed ground truth. DerivedLabeler scores 75% name
specificity, 90% lexical groundedness, and 0% sibling collisions. Generic Core,
Components, Fragments and [page] names account for specificity failures.

Reproduction instructions: `codearch/eval/README.md`. Auditable artifacts:
`codearch/eval/results-agent/report.json`, its `report.md`, and
`codearch/eval/labels-real-report.md`. The runnable M1 numeric exit exists; the
result does not support automatically loading the full map for simple file lookup.
No M2+ production features were added. 44 Rust tests and 10 harness tests pass.
