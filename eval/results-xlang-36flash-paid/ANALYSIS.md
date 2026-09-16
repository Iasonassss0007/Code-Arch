# Cross-language impact, gemini-3.6-flash, paid tier (2026-09-16)

45/48 episodes, 8 tasks (`tasks-xlang.json`), 3 arms x 2 trials. Stopped by
`--max-cost 7` at $7.05 list price (upper bound; implicit caching not deducted).
Missing: 3 episodes of `sharelinkviewset-6`/`uisettingsview-7` trial 1.

| Arm | Episodes | Mean F1 | Exact | Files opened | importers lookups | List USD |
|---|---:|---:|---:|---:|---:|---:|
| without_map | 15 | **0.809** | 4 | 30 | 0 | 2.72 |
| with_map | 15 | 0.691 | 3 | 31 | 0 | 2.57 |
| importers_tool | 15 | 0.717 | 3 | 19 | 17 | 1.75 |

Paired per-task delta vs without_map, 7 tasks with both trials in every arm (95%
bootstrap CI): with_map -0.096 [-0.46, +0.27]; importers_tool -0.098 [-0.43, +0.26].
Per-task deltas swing from -0.85 to +0.86.

## Reading

- Unlike flash-lite (`results-xlang/`), this model explores and uses the tool
  (17 lookups in 15 episodes), so the comparison is meaningful.
- **Neither the map nor the importers tool beats no map on cross-language impact.**
  No CI excludes zero; the point estimates favour no map.
- Likely cause: the coupling here is a URL string (Django route <-> Angular
  `HttpClient` call), which the static import index does not contain. The tool
  answers only the second hop (direct importers of the caller); the agent must still
  find the caller by search, which it does equally well without help. Contrast
  `results-nav-importers/`: importers_tool +0.37 F1 when the dependency *is* an import.
- importers_tool was the cheapest arm (-36% list cost vs without_map): fewer opens.

## Harness defect (fixed after this run)

`gemini_agent.ACTION_SCHEMA` offered `importers` in the sampler enum to every arm.
without_map and with_map each called it on 2 episodes (`bulkeditobjectsview-1`,
`processedmailviewset-5`, both trials), scoring F1 0. The enum is now scoped to the
arm whose system prompt includes the tool (`schema_for`). The defect only handicapped
the no-tool arms, so correcting it cannot overturn "no benefit over without_map".

## Next

Serve the cross-language edge itself: expose Code Arch's URL contract join
(backend view -> frontend callers) as a tool, and rerun this task set against it.
