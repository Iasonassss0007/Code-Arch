# Route join: cut common-word false positives

Date: 2026-09-16. Status: ready to implement. No model calls, no spend.

## Goal

Raise `contract::route_join` precision on paperless-ngx from **0.78** without losing
anything the join finds today. Callers of `.codearch/routes.md` (the eval's
`route_callers` tool, and any agent reading the file) trust every listed file, so a false
caller costs more than a missed one.

## Baseline (commit `027049d`)

Measured against the xlang oracle (`eval/build_tasks_xlang.py`, 43 views with callers):

| Metric | Value |
|---|---:|
| Views with callers | 45 |
| True positives | 45 |
| False positives | 13 |
| False negatives | 0 |
| Precision / recall | 0.78 / 1.00 |
| Eval tasks exact (`tasks-xlang.json`) | 8/8 |

## Acceptance gates

All must hold on the final change:

1. Recall stays **1.00** (0 false negatives).
2. `tasks-xlang.json`: **8/8** views still exact.
3. False positives **≤ 5** (precision ≥ 0.90). Report the actual number either way.
4. `cargo test` and `python -m pytest -q eval/test_*.py` pass.
5. No paperless-specific names in the code (`resourceName`, `apiBaseUrl`,
   `AbstractPaperlessService`, `getResourceUrl`). The oracle already encodes those
   conventions; copying them would make the join agree with the oracle by construction.
   Rules must be generic TypeScript/HTTP-client rules.

## The 13 false positives and their causes

Each was traced to the literal that produced the matching segment.

| View (route key) | False caller | Literal that matched | Cause |
|---|---|---|---|
| `TasksViewSet` (`tasks`) | `app-frame.component.ts` | `import … from 'src/app/services/tasks.service'` | A. import specifier |
| `serve_logo` (`logo`) | `app-frame.component.ts` | `import … from '../common/logo/logo.component'` | A. import specifier |
| `TagViewSet` (`tags`) | `document-detail.component.ts` | `import … from '../common/input/tags/tags.component'` | A. import specifier |
| `serve_logo` (`logo`) | `logo.component.ts` | `['logo'].concat(...)` (CSS class) | B. non-URL word |
| `CustomFieldViewSet` (`custom_fields`) | `document-detail.component.ts` | `this.documentForm.get('custom_fields')` | B. form control name |
| `UnifiedSearchViewSet` (`documents`) | `document-detail.component.ts` | `this.router.navigate(['documents', id, …])` | B. client-side router |
| `UnifiedSearchViewSet` (`documents`) | `trash.service.ts` | `data['documents'] = documents` | B. object key |
| `GlobalSearchView` (`search`) | `document.service.ts` | `` `#search="…"` `` URL fragment (l.221) | B. non-URL word (confirmed) |
| `SystemStatusView` (`status`) | `share-link-bundle-dialog.component.ts` | `ShareLinkBundleSummary['status']` indexed-access key (l.109) | B. non-URL word (confirmed) |
| `UnifiedSearchViewSet` (`documents`) | `chat.service.ts` | `` `${…}documents/chat/` `` | C. prefix of a longer route |
| `SharedLinkView` (`share`) | `share-links-dialog.component.ts` | `.replace(/\/api\/$/, '/share/')` | D. URL built for a browser page |
| `SharedLinkView` (`share`) | `share-link-bundle-dialog.component.ts` | same | D |
| `SharedLinkView` (`share`) | `share-link-bundle-manage-dialog.component.ts` | same (l.96, confirmed) | D |

Rows marked *verify* were inferred from the segment, not yet traced to a line. Confirm
them in step 0 before designing around them.

## Plan

Work in this order. Measure after **each** change and record the numbers in the log at
the end of this file. Keep a change only if gates 1, 2 and 4 still hold.

### Step 0: add the scorer and measure the baseline

Create `eval/score_routes.py`. It reads a `routes.md`, scores it against the oracle, and
prints per-view FP/FN plus a summary line. It parses only the markdown; it shares no code
with `src/contract.rs`.

```python
"""Score a codearch routes.md against the xlang oracle (view -> direct URL callers)."""
import argparse, json, re
from pathlib import Path
import build_tasks_xlang as X

def parse(text):
    out = {}
    for sec in text.split('\n## ')[1:]:
        view = re.match(r'`(\w+)`', sec).group(1)
        out[view] = set(re.findall(r'^- `([^`]+)`', sec, re.M))
    return out

def score(pred, gold):
    tp = fp = fn = 0
    rows = []
    for v in sorted(set(gold) | set(pred)):
        g, p = gold.get(v, set()), pred.get(v, set())
        tp += len(g & p); fp += len(p - g); fn += len(g - p)
        rows += [('FP', v, f) for f in sorted(p - g)] + [('FN', v, f) for f in sorted(g - p)]
    return tp, fp, fn, rows

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('routes', type=Path)
    args = ap.parse_args()
    pred = parse(args.routes.read_text(encoding='utf-8'))
    gold, _ = X.callers_by_view(X.routes(), X.frontend())
    gold = {v: f for (v, _), f in gold.items()}
    tp, fp, fn, rows = score(pred, gold)
    for kind, v, f in rows:
        print(kind, v, f)
    tasks = json.loads((X.ROOT / 'tasks-xlang.json').read_text(encoding='utf-8'))
    exact = sum(pred.get(t['view'], set()) == set(t['oracle']['direct_callers']) for t in tasks)
    print(f'P={tp/max(tp+fp,1):.2f} R={tp/max(tp+fn,1):.2f} tp={tp} fp={fp} fn={fn} '
          f'tasks_exact={exact}/{len(tasks)}')

if __name__ == '__main__':
    main()
```

Add one hermetic test for `parse` and `score` (a two-view markdown string and a
hand-written gold dict) to `eval/test_tasks_xlang.py`.

Measure. `--codearch-dir` keeps the output **out of the pinned checkout**; the eval runner
refuses a dirty `eval/repos/paperless-ngx`.

```bash
cargo build
S=$(mktemp -d)
./target/debug/codearch eval/repos/paperless-ngx --out "$S/map.md" --no-index --codearch-dir "$S/state"
cd eval && python score_routes.py "$S/state/routes.md"
git -C repos/paperless-ngx status --porcelain   # must print nothing
```

Expected: `P=0.78 R=1.00 tp=45 fp=13 fn=0 tasks_exact=8/8`. If it differs, stop and find
out why before changing anything. Then trace the three *verify* rows to their lines and
update the table.

### Step 1: exclude module specifiers (cause A, expect −3)

In `src/parse.rs`, the TS literal scan in `visit` reads every `string` node, including the
`source` of import/export statements and the argument of `require()` / `import()`. Those
are module paths, not URLs.

- Skip a `string` node that is the `source` of a node in `IMPORT_NODES`, or the specifier
  `import_call_specifier` already recognizes.
- Unit test in `parse.rs`: a file containing only
  `import { X } from '../common/tags/tags.component'` has no `tags` in `url_segments`;
  adding `resourceName = 'tags'` brings it back.

### Step 2: skip literals in non-request positions (cause B, expect −4 to −6)

Most remaining false segments are words used as keys or client-side paths, not request
URLs. Generic rules, in order of confidence. Implement one at a time and measure each:

1. **Subscript keys**: `obj['documents']` (a `string` directly inside a
   `subscript_expression`).
2. **Client-side router arguments**: literals inside the array argument of a
   `.navigate(` call, the argument of `.navigateByUrl(`, and values of `path:`,
   `redirectTo:` and `routerLink` properties. Generic to Angular Router; React Router's
   `path:` is covered too.
3. **Form and DOM accessors**: first argument of `.get(`, `.getElementById(`,
   `.querySelector(`, `.addControl(`, `.removeControl(`, `.setControl(`. Careful:
   `this.http.get(url)` is a request, so only skip `.get(` when the receiver text does
   **not** contain `http`. Test both cases.

Do not add a stopword list of English words: `tags`, `documents` and `logs` are real route
keys that services pass as bare strings.

### Step 3: most-specific route wins within one literal (cause C, expect −1)

`` `${base}documents/chat/` `` matches both `documents/chat` and `documents`. Segments are a
flat set per file today, so the join cannot tell that `documents` only ever appears as
part of `documents/chat`.

- Keep, per TS file, the segment **sequence** of each literal (`["documents","chat"]`)
  alongside the flat set. This changes `FileParse`: bump `cache::FORMAT` to 5.
- In `route_join`: a file matches route key K unless **every** literal containing K's
  segments also matches a strictly longer route key that extends K. Bare single-word
  literals (`'tags'`) still match, so services that spread a URL over fields keep working.
- Check `document.service.ts` stays a caller of `UnifiedSearchViewSet` (a true positive
  through its inherited base URL), then run the scorer.

If this rule costs any recall, revert it and record why. One false positive is not worth a
recall loss.

### Step 4: page URLs vs API URLs (cause D, investigate only)

The share dialogs build a browser link by rewriting `/api/` to `/share/`. The Django route
`share/<slug>` is a real server route, so the join is arguably right that these files
reference it; the oracle only counts `/api/` requests. Decide whether this is a join error
or an oracle scope choice, and write the decision in this file. Change code only if the fix
is generic and cheap (e.g. literals passed to `.replace(` are pattern rewrites, not
requests).

**Decision (2026-09-16, measured): join error under `routes.md`'s own contract, fixed.**
`routes.md` promises "frontend files whose *request strings* name every segment", and the
`route_callers` consumer asks which files *send requests* to a view. No HTTP is ever sent
to `/share/` from these files — `'/share/'` is the replacement pattern of
`pathname.replace(/\/api\/$/, '/share/')`, and the result feeds `navigator.share` /
clipboard (a browser page link), not a request. Listing page-link builders as API callers
is exactly the false-caller cost the goal statement worries about, so the three
`SharedLinkView` rows are join errors, not just oracle scope. The fix — skip string
arguments of `.replace(` (one entry in the existing non-request-methods list; the string
being rewritten still counts) — is generic (`String.prototype`, no repo names) and cheap.
Measured: FP 5 → 2, recall 1.00, tasks 8/8. The files still reference the server route in
a page-link sense; if a future edge wants page-link dependents, that is a different claim
than `route_join` makes.

**Extra (not in the plan above): `#`-fragment literals.** `document.service.ts` line 221
sets `url.hash = `#search="…"` — a fragment, never a request path (HTTP never sends one).
Same literal-shape family as the existing space-free rule, one line, recall-safe by
construction. Measured: FP 2 → 1, recall 1.00, tasks 8/8. Kept.

**Known limit left standing:** `serve_logo` ← `logo.component.ts`. Its `logo` segments
come from `['logo'].concat(...).join(' ')` (a CSS class list) plus `templateUrl` /
`styleUrls` file metadata. Suppressing those needs dataflow (array → `.join(' ')`) or
framework-metadata keys, both narrower than every rule above; one FP is not worth the
tuning risk. Recorded in the `route_join` doc comment.

## Rules while working

- **One change, one measurement.** Record every attempt in the log below, including failed
  ones, so the same dead end is not tried twice.
- **Stop rule:** if two different attempts at the same cause both lose recall, stop on that
  cause, record it as a known limit in the `route_join` doc comment, and move on.
- **Do not tune against the 8 eval tasks.** Optimize the 43-view numbers; the 8 tasks are a
  gate, not a target.
- Update the precision/recall numbers in the `route_join` doc comment (`src/contract.rs`)
  to the final measurement.
- Run `codearch` on the other eval repos (`hono`, `commerce`, `typedi`, `realworld`) before
  and after, each with `--codearch-dir` outside the checkout. `routes.md` should stay absent
  on the TS-only repos, and `realworld` should not lose route links. Note any change here.
- Commit per step; the message states the before → after false-positive count.

## Measurement log

| Step | Change | tp | fp | fn | P | R | Tasks exact | Kept? |
|---|---|---:|---:|---:|---:|---:|---:|---|
| 0 | baseline | 45 | 13 | 0 | 0.78 | 1.00 | 8/8 | — |
| 0 | scorer + verify traces (no code change) | 45 | 13 | 0 | 0.78 | 1.00 | 8/8 | — |
| 1 | exclude module specifiers | 45 | 10 | 0 | 0.82 | 1.00 | 8/8 | kept |
| 2 | all of step 2 combined | 45 | 6 | 0 | 0.88 | 1.00 | 8/8 | kept |
| 2-ablate | step 2 without subscript-index rule | 45 | 7 | 0 | 0.87 | 1.00 | 8/8 | — (subscript rule = −1: trash.service `data['documents']`; `SystemStatusView` FP already gone there via the `literal_type` half, so indexed-access `T['status']` = −1) |
| 2-ablate | step 2 without `path:`-pair rule | 45 | 6 | 0 | 0.88 | 1.00 | 8/8 | — (`path:`-keys −0 on paperless; kept as generic router rule with unit test) |
| 2-ablate | step 2 without call-arg rules (navigate + accessors) | 45 | 8 | 0 | 0.85 | 1.00 | 8/8 | — (navigate = −1 doc-detail `['documents']`, accessors = −1 doc-detail `.get('custom_fields')`) |
| 3 | most-specific route wins (`url_segment_seqs`, FORMAT 5) | 45 | 5 | 0 | 0.90 | 1.00 | 8/8 | kept (−1: chat.service `` `…documents/chat/` ``; still caller of `ChatStreamingView`, doc.service still caller of `UnifiedSearchViewSet`) |
| 4 | skip `.replace(` pattern args (cause D: join error, see decision) | 45 | 2 | 0 | 0.96 | 1.00 | 8/8 | kept (−3: share dialogs; `SharedLinkView` section drops, oracle agrees it has no `/api/` caller) |
| extra | skip `#`-fragment literals (`` url.hash = `#search=…` ``) | 45 | 1 | 0 | 0.98 | 1.00 | 8/8 | kept (−1: doc.service `#search`; request paths never start with `#`) |

Other repos (before → after, `--codearch-dir` outside each checkout, checkouts verified
clean): `hono`, `commerce`, `typedi`, `realworld` all have no `routes.md` at baseline
(`027049d` worktree build) and none after — no route links lost or gained. (`realworld`
here is backend-only for route purposes: 37 files, no TS frontend callers.)
| | | | | | | | | |
