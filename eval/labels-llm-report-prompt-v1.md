# Local model vs derived labeler

Model: `qwen2.5-coder-1.5b-instruct-q4_k_m.gguf`. 20 frozen reference clusters. 0 of 20 fell back to the derived name (generation failure, ungrounded output, collision, or bad length).

Higher is better for specificity and groundedness; lower is better for collisions. References are model-reviewed, not human ground truth, so these numbers compare two labelers against a fixed standard rather than establishing absolute quality.

| Metric | Derived | Local model |
|---|---:|---:|
| Name specificity | 75% | 40% |
| Lexical groundedness | 90% | 40% |
| Sibling collisions | 0% | 0% |

## Per-cluster names

| Cluster | Derived | Local model | Changed |
|---|---|---|---|
| hono-0 | Core | src | yes |
| hono-1 | Jsx | Jsx |  |
| hono-2 | Routers | PatternRouter | yes |
| hono-3 | Ssg | Adapter | yes |
| hono-4 | Jwt | utils | yes |
| hono-5 | Streaming | Streaming |  |
| hono-6 | Routers Deno | Router | yes |
| hono-7 | React Jsx | BuildSystem | yes |
| hono-8 | Benchmarks Jsx | Benchmarks Jsx |  |
| hono-9 | Etag | Middleware | yes |
| commerce-0 | Fragments | Fragments |  |
| commerce-1 | Filter | Filter |  |
| commerce-2 | Components | Components |  |
| commerce-3 | Search | Search |  |
| commerce-4 | Core | Core |  |
| commerce-5 | Cart | Cart |  |
| commerce-6 | [page] | [page] |  |
| commerce-7 | Navbar | Navbar |  |
| commerce-8 | [collection] | [collection] |  |
| commerce-9 | Revalidate | API | yes |
