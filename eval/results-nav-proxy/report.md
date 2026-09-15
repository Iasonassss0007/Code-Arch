# M1v2 offline efficacy proxy — iterative grep vs import index

Deterministic information-access measurement, zero provider calls. Not an LLM agent run;
the paid M1v2 run stays the real gate. Pre-stated rule: index natural F1 >= grep + 0.10
AND index mean tokens <= grep mean tokens.

Tasks 37 · counter cl100k_base via eval-support · grep budget 15 searches / 40 opens (exhausted on 14)

| Arm | Natural F1 | Cardinality-controlled F1 | Mean tokens | Mean opens | Mean searches |
|---|---:|---:|---:|---:|---:|
| Iterative grep | 0.279 | 0.287 | 46754 | 29.4 | 8.9 |
| Map + index | 0.890 | 0.928 | 17630 | 1.0 | 0.0 |

Offline payoff: yes.

Per-task: grep found vs index found vs expected, with the one-shot free-adversary F1
the task was admitted against, so the iterative-vs-one-shot baseline difference is visible.

| Task | Repo | Expected | Grep found | Index found | Grep F1 | Index F1 | Adversary F1 |
|---|---|---:|---:|---:|---:|---:|---:|
| hono-impact-handler-58 | hono | 19 | 4 | 19 | 0.000 | 1.000 | 0.000 |
| hono-impact-compress-56 | hono | 17 | 8 | 17 | 0.080 | 1.000 | 0.211 |
| hono-impact-concurrent-57 | hono | 13 | 15 | 13 | 0.615 | 1.000 | 0.143 |
| hono-impact-index-23 | hono | 18 | 7 | 18 | 0.000 | 1.000 | 0.056 |
| hono-impact-index-60 | hono | 10 | 7 | 10 | 0.000 | 1.000 | 0.300 |
| hono-impact-index-26 | hono | 7 | 7 | 7 | 0.000 | 1.000 | 0.000 |
| hono-impact-index-36 | hono | 18 | 7 | 18 | 0.000 | 1.000 | 0.000 |
| hono-impact-accept-53 | hono | 10 | 9 | 10 | 0.211 | 1.000 | 0.182 |
| hono-impact-color-55 | hono | 9 | 8 | 9 | 0.118 | 1.000 | 0.182 |
| hono-impact-websocket-19 | hono | 5 | 12 | 5 | 0.000 | 1.000 | 0.200 |
| hono-impact-page-1 | hono | 4 | 7 | 4 | 1.000 | 1.000 | 0.000 |
| hono-impact-conninfo-16 | hono | 3 | 23 | 3 | 0.000 | 1.000 | 0.333 |
| hono-impact-conninfo-20 | hono | 3 | 23 | 3 | 0.000 | 1.000 | 0.333 |
| hono-impact-conninfo-7 | hono | 3 | 23 | 3 | 0.000 | 1.000 | 0.333 |
| hono-impact-index-25 | hono | 3 | 7 | 3 | 0.000 | 1.000 | 0.333 |
| hono-impact-serve-static-module-12 | hono | 3 | 8 | 3 | 0.333 | 1.000 | 0.000 |
| hono-impact-ssg-18 | hono | 3 | 14 | 3 | 0.333 | 1.000 | 0.333 |
| hono-impact-websocket-15 | hono | 3 | 12 | 3 | 0.000 | 1.000 | 0.333 |
| hono-impact-handler-21 | hono | 2 | 4 | 2 | 0.000 | 1.000 | 0.000 |
| hono-impact-index-49 | hono | 2 | 7 | 3 | 0.000 | 0.500 | 0.000 |
| hono-impact-index-51 | hono | 2 | 7 | 6 | 0.000 | 0.000 | 0.000 |
| hono-impact-jsx-dev-runtime-39 | hono | 2 | 16 | 2 | 0.500 | 1.000 | 0.000 |
| hono-impact-page-preact-4 | hono | 2 | 4 | 2 | 0.500 | 1.000 | 0.000 |
| hono-impact-page-react-0 | hono | 2 | 4 | 2 | 0.500 | 1.000 | 0.000 |
| hono-impact-page-react-5 | hono | 2 | 4 | 2 | 0.500 | 1.000 | 0.000 |
| hono-impact-serve-static-17 | hono | 2 | 14 | 2 | 0.000 | 1.000 | 0.000 |
| hono-impact-serve-static-8 | hono | 2 | 14 | 2 | 0.500 | 1.000 | 0.000 |
| hono-impact-ssg-10 | hono | 2 | 14 | 2 | 0.500 | 1.000 | 0.000 |
| commerce-impact-image-5 | commerce | 7 | 4 | 30 | 0.000 | 1.000 | 0.286 |
| commerce-impact-price-3 | commerce | 3 | 5 | 14 | 0.333 | 0.333 | 0.000 |
| commerce-impact-product-6 | commerce | 6 | 0 | 29 | 0.000 | 1.000 | 0.333 |
| commerce-impact-cart-4 | commerce | 3 | 0 | 26 | 0.000 | 1.000 | 0.000 |
| commerce-impact-label-0 | commerce | 2 | 1 | 9 | 0.667 | 0.500 | 0.000 |
| typedi-impact-resolve-to-type-wrapper-3 | typedi | 19 | 18 | 19 | 0.973 | 1.000 | 0.158 |
| typedi-impact-cannot-inject-value-2 | typedi | 19 | 18 | 19 | 0.973 | 1.000 | 0.250 |
| typedi-impact-inject-1 | typedi | 17 | 16 | 17 | 0.970 | 1.000 | 0.182 |
| typedi-impact-inject-many-0 | typedi | 17 | 17 | 17 | 1.000 | 1.000 | 0.182 |
