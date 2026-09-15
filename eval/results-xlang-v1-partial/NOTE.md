# Superseded partial run (2026-09-15)

48/108 episodes on the first 18-task `tasks-xlang.json` (trial 0 only), stopped by a
Gemini free-tier HTTP 429 (daily quota). Kept as the evidence for the `search_guess`
gate: one class-name search found a real caller in 14/16 without_map episodes, because
paperless names both sides alike (`TagViewSet` / `tag.service.ts`). The gate cut the set
to 8 tasks; this checkpoint's config no longer matches and cannot be resumed.

Trial-0 F1 on 16 tasks: without_map 0.492, with_map 0.535, importers_tool 0.604 (1 lookup
in 15 episodes). On the 8 surviving tasks: 0.387 / 0.423 / 0.496 (7 episodes each).
