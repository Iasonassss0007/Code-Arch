# Parity baseline

This directory is the index-only output of codearch `6503435` on four pinned repositories.
Diff a new index against these files to see what a later change moved.

The recorded commit is `git rev-parse --short HEAD` before the baseline commit.
The script keeps that hash when a later commit only adds `eval/baseline/` or `eval/make_baseline.py`.

## Pins

- hono `e7b38ee42bfa41f20194ad20fb949b625346be62`
- commerce `3761e52e60df9c6a316e067dbfd7032e494d3634`
- typedi `7c6ac12a3de8507b1306f761942562f15375ed1f`
- paperless-ngx `1f374cd65652ff8991f1d7e1efe49354876ac5c2`

The clones live in `eval/repos/` and are gitignored.
Each clone must be clean and at the pin above.

## Files

Each repository directory has `imports.md` and `report.txt`.
`routes.md` is present when codearch writes one.
`report.txt` is the index stdout.
The temporary `--codearch-dir` path in that stdout is rewritten to `eval/baseline/<repo>`.
`routes-score.md` is the paperless route score.

## Regenerate

From the repository root, clone each pin if `eval/repos/<name>` is missing.

```
git clone --filter=blob:none https://github.com/honojs/hono.git eval/repos/hono
git -C eval/repos/hono checkout e7b38ee42bfa41f20194ad20fb949b625346be62
git clone --filter=blob:none https://github.com/vercel/commerce.git eval/repos/commerce
git -C eval/repos/commerce checkout 3761e52e60df9c6a316e067dbfd7032e494d3634
git clone --filter=blob:none https://github.com/typestack/typedi.git eval/repos/typedi
git -C eval/repos/typedi checkout 7c6ac12a3de8507b1306f761942562f15375ed1f
git clone --filter=blob:none https://github.com/paperless-ngx/paperless-ngx.git eval/repos/paperless-ngx
git -C eval/repos/paperless-ngx checkout 1f374cd65652ff8991f1d7e1efe49354876ac5c2
```

Then rebuild this directory.

```
python eval/make_baseline.py
```

The script exits if a clone is missing, dirty, or on the wrong commit.
The script indexes with `codearch eval/repos/<repo> --codearch-dir <tmp>`.
It does not write into the clone.
The import-index check writes its report to a temporary directory.
`eval/results-nav-ceiling` stays unchanged.

## Compare a later index

Diff the new index against the recorded file.

```
git diff --no-index eval/baseline/<repo>/imports.md <new>
```

Replace `<repo>` with one pin name.
Do the same for `routes.md` when that repository has one.

## Query parity

```
parity: 42/42 lookups match (0 mismatches)
```

## Import index

```
# M1v2 deterministic ceiling — reverse import index

Transitive importers walked from `.codearch/imports.md` alone, scored against madge. No model: this bounds what any agent can get from the index.

Tasks 34 · mean recall 1.000 · mean F1 0.974 · mean F1 in madge scope 0.974 · full recall 34/34 · free-adversary mean F1 0.124

Gate (mean recall ≥ 0.80): pass
```
