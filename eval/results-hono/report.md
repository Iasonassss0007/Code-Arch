# M1 evaluation result

Fixed-policy navigation benchmark; not evidence of LLM coding-agent improvement. See task provenance for corpus scope.

Policy: lexical navigation proxy v1

| Arm | Correct | Tokens | Files opened | Searches |
|---|---:|---:|---:|---:|
| without_map | 0/12 | 159130 | 12 | 12 |
| with_map | 2/12 | 86570 | 12 | 0 |

Token savings: 45.60%. Accuracy delta: 16.67 percentage points.
Map helps under this policy: True.

Tokens include the complete map on every mapped episode, file inventory, task, tool outputs and actions. They are context volume, not billed model usage.

Labels: 20 authored cases. Specificity 100%; lexical groundedness 100%; sibling collisions 0%.

These are authored synthetic clusters, not independently human-reviewed annotations. Groundedness is a conservative lexical traceability check, not a semantic or grammatical noun classifier.

No real-agent efficacy claim is supported. Run an LLM adapter before using this result to justify M2 or later breadth.
