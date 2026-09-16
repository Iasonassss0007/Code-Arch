# routes_tool vs without_map, gemini-3.6-flash, paid (2026-09-16)

16/16 episodes, 8 tasks x 1 trial. $1.61 list price.

| Arm | Mean F1 | Exact | Files opened | importers | route_callers | List USD |
|---|---:|---:|---:|---:|---:|---:|
| without_map | **0.874** | 2/8 | 14 | 0 | 0 | 1.14 |
| routes_tool | 0.703 | 2/8 | 3 | 9 | 8 | 0.47 |

Paired delta -0.171 [-0.40, +0.05]; 2 wins, 4 losses, 2 ties.

The model used the new tool exactly as intended on every task (route_callers, then
importers), and route_callers returned the exact oracle callers 8/8. The loss is the
second hop: **codearch's import index held 46% of madge's frontend edges.** paperless
imports `'src/app/services/...'` through `src-ui/tsconfig.json` `baseUrl`, and profile
discovery read only root and workspace tsconfigs. E.g. `correspondent.service.ts`: index
listed 3 importers, madge 16; the agent trusted the tool and answered 6 of 19 files.

Fixed after this run (profile `nested_ts_apps`): index recall vs madge 0.457 -> 1.000
(2,120 edges). A perfect tool user (route_callers + depth-1 importers) scores F1 1.00 on
all 8 tasks with the fixed index, 0.27-0.94 before. The same defect handicapped
importers_tool in `results-xlang-36flash-paid/`. Rerun of routes_tool on the fixed index:
`results-xlang-routes-fixed/`.
