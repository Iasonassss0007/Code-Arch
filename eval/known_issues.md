# Known issues

## A tsconfig `extends` target outside the repository

Freshness tracks `tsconfig.json` and `jsconfig.json` inside the repository. It also tracks an `extends` target when that file stays inside the repository.

`codearch` does not walk `node_modules`. An `extends` target there, or any path outside the repository, is not part of the freshness snapshot. A change to that file can leave `codearch importers` printing `index: fresh` while the path aliases are out of date. Run `codearch` again after you change that config.
