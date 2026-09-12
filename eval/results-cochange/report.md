# Git co-change — what it changed

Each checkout rendered twice, `--no-git` against the default. Pairs are
unordered file pairs sharing a domain. A grouped pair with no import edge
is coupling the import graph never saw; whether that is signal or noise is
not decided here.

| Repo | Commits | Pairs | Domains | Confidence | Grouped | Separated | Grouped w/o import |
|---|---|---|---|---|---|---|---|
| hono | 555 | 352 | 16 → 16 | 0.94 → 0.88 | 0 | 0 | 0 |
| commerce | 24 | 120 | 14 → 10 | 0.86 → 0.79 | 42 | 0 | 42 |
| typedi | 5 | 1 | 6 → 6 | 0.97 → 0.97 | 0 | 0 | 0 |

## Confidence bands

| Repo | Without git | With git |
|---|---|---|
| hono | 16 high, 0 medium, 0 low | 16 high, 0 medium, 0 low |
| commerce | 10 high, 3 medium, 1 low | 7 high, 2 medium, 1 low |
| typedi | 6 high, 0 medium, 0 low | 6 high, 0 medium, 0 low |

## Groupings the import graph does not explain

**hono** — 0 pairs, first 5:

- none

**commerce** — 42 pairs, first 5:

- `app/[page]/layout.tsx` ↔ `app/search/[collection]/opengraph-image.tsx`
- `app/[page]/layout.tsx` ↔ `app/search/[collection]/page.tsx`
- `app/[page]/layout.tsx` ↔ `app/search/children-wrapper.tsx`
- `app/[page]/layout.tsx` ↔ `app/search/layout.tsx`
- `app/[page]/layout.tsx` ↔ `app/search/loading.tsx`

**typedi** — 0 pairs, first 5:

- none

