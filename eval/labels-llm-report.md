# Local model vs derived labeler

Model: `qwen2.5-coder-1.5b-instruct-q4_k_m.gguf`. 20 frozen reference clusters. 0 of 20 fell back to the derived name (generation failure, ungrounded output, collision, or bad length); 15 kept the generated name with a derived summary (summary guard).

Higher is better for specificity and groundedness; lower is better for collisions. References are model-reviewed, not human ground truth, so these numbers compare two labelers against a fixed standard rather than establishing absolute quality.

| Metric | Derived | Local model |
|---|---:|---:|
| Name specificity | 75% | 60% |
| Lexical groundedness | 90% | 100% |
| Sibling collisions | 0% | 0% |

## Per-cluster names

| Cluster | Derived | Local model | Changed |
|---|---|---|---|
| hono-0 | Core | src | yes |
| hono-1 | Jsx | Jsx |  |
| hono-2 | Routers | Router | yes |
| hono-3 | Ssg | SSG | yes |
| hono-4 | Jwt | Jwt |  |
| hono-5 | Streaming | Streaming |  |
| hono-6 | Routers Deno | Routers Deno |  |
| hono-7 | React Jsx | buildPage | yes |
| hono-8 | Benchmarks Jsx | Content | yes |
| hono-9 | Etag | Middleware | yes |
| commerce-0 | Fragments | fragments | yes |
| commerce-1 | Filter | Filter |  |
| commerce-2 | Components | Grid | yes |
| commerce-3 | Search | Search |  |
| commerce-4 | Core | Layout | yes |
| commerce-5 | Cart | Cart |  |
| commerce-6 | [page] | [page] |  |
| commerce-7 | Navbar | Navbar |  |
| commerce-8 | [collection] | [collection] |  |
| commerce-9 | Revalidate | Revalidate |  |
