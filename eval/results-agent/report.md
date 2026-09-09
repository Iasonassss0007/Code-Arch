# M1 real-agent evaluation

Model: `qwen/qwen3.5-flash-02-23`. 24 tasks, 2 fresh trials per arm.

| Arm | Correct | Context tokens | Provider tokens | Files opened | Searches | USD |
|---|---:|---:|---:|---:|---:|---:|
| without_map | 46/48 | 159821 | 304432 | 51 | 2 | 0.020140 |
| with_map | 45/48 | 289899 | 605301 | 53 | 2 | 0.039704 |

Context-token savings: -81.39%; provider-token savings: -98.83%; accuracy delta: -2.08 pp.

Context counts each observation/action once; provider usage includes repeated conversation input and system prompt. Both include map cost. Provider-reported USD includes any caching effects.

This is a small file-location benchmark, not a code-change evaluation. Repeated trials on the same task are correlated. Tier 3 uses TypeDI runtime/decorator indirection, not a large enterprise application. Labels require separate review.
