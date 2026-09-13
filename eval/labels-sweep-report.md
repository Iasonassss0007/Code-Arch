# Multi-model sweep vs derived labeler

20 frozen reference clusters, metric v2 (stemmed, guard-enforced, prompt v2). References are model-reviewed, not human ground truth, so these numbers compare labelers against a fixed standard rather than establishing absolute quality. Counts sit beside percentages: on 20 clusters one point is one cluster.

| Model | Specificity | Groundedness | Collisions | Name FB | Summary FB | Time (s) |
|---|---:|---:|---:|---:|---:|---:|
| derived (baseline) | 75% (15/20) | 90% | 0% | - | - | 0.4 |
| qwen25-coder-1.5b | 60% (12/20) | 100% | 0% | 0/20 | 15/20 | 92 |
| qwen25-coder-0.5b | 20% (4/20) | 80% | 10% | 0/20 | 15/20 | 57 |
| llama32-1b | 50% (10/20) | 90% | 0% | 0/20 | 9/20 | 57 |
| gemma2-2b-it | 20% (4/20) | 35% | 0% | 0/20 | 14/20 | 120 |

Rule winner: `qwen25-coder-1.5b`. Gap to derived specificity: -15 pp.

## Reproduce

```powershell
cargo build --features llm
python eval/score_sweep.py
```
