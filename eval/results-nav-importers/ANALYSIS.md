# importers_tool live run — analysis (2026-09-15)

Follow-up to `results-nav-3arm/ANALYSIS.md`. Same model (`gemini-3.5-flash-lite`),
same 37 gated tasks × 2 trials. Only the `importers_tool` arm ran here: no map
text, plus a `{"tool":"importers","path":…}` action that returns transitive importers
with hop depth from Code Arch's import index. There are 74/74 episodes and 0 errors.
The comparison rows come from the 3-arm checkpoint.

The run's own `report.json`/`report.md` are missing because the report step crashed on a
single-arm run (it assumed the without_map/with_map pair; now fixed in
`run_agent.py`). `--resume` then correctly refused, because the harness hash in the
config changed. All numbers below are computed from `checkpoint.json`.

| Arm | Mean F1 | Exact | Precision | Recall | Answer size | Provider tokens/ep | Errors |
|---|---:|---:|---:|---:|---:|---:|---:|
| without_map | 0.577 | 12/74 | 0.749 | 0.632 | 4.8 | 10,820 | 2 |
| with_map | 0.539 | 4/74 | 0.658 | 0.613 | 7.5 | 20,080 | 0 |
| index_only | 0.515 | 12/74 | 0.667 | 0.544 | 4.9 | 11,783 | 8 |
| **importers_tool** | **0.890** | **60/74** | 0.865 | **1.000** | 9.6 | **8,868** | 0 |

Paired per-task delta vs without_map (5000-sample bootstrap):

| Slice | n | ΔF1 | 95% CI | tasks better/worse |
|---|---:|---:|---|---:|
| all | 37 | +0.313 | [+0.202, +0.424] | 28/2 |
| T1 (Hono) | 28 | +0.333 | [+0.235, +0.438] | 22/0 |
| T2 (Commerce) | 5 | −0.155 | [−0.417, +0.022] | 2/2 |
| T3 (TypeDI) | 4 | +0.760 | [+0.686, +0.805] | 4/0 |

## Reading

1. **Serving the index as a query works.** It's the first significant result in M1v2: +0.31 F1
   and 60 exact vs 12, using 18% fewer provider tokens than no map and 56% fewer than
   with_map. The agent makes one lookup and answers, with no opens or searches.
2. **The agent passes the tool's output through unchanged.** F1 0.890 equals the scripted
   one-call ceiling and the offline proxy. What limits the score now is the index, not the agent.
3. **The Commerce "losses" come from wrong expected sets.** madge doesn't resolve
   Commerce's bare `baseUrl` imports (`components/grid/tile`). Checked chain:
   `app/page.tsx` → `components/grid/three-items` → `components/grid/tile` →
   `../label`. That makes `app/page.tsx` a real transitive importer of `label.tsx`,
   but the oracle lists only `carousel.tsx` and `tile.tsx`. The agent's larger
   Commerce answers (9–30 files) are most likely correct. The Commerce expected sets
   need a resolver that honors `tsconfig` `baseUrl`/`paths` before T2 can be scored.
4. **This mostly measures the index, not reasoning.** When the tool answers the
   question, the result shows that the delivery works, not that the model is smart.
   The open question moves to tasks a static import walk can't answer
   (DI, URL contracts, "where does X belong").

## Next

- Fix the oracle: regenerate Commerce expected sets with `baseUrl`/`paths` resolution, then rescore both checkpoints.
- Build a task set where the static import walk is wrong or incomplete, to test the map on what the tool can't cover.
