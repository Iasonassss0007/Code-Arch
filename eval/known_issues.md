# Known issues

## `parse::parse_all` returns empty parses

`parse::parse_all(inv)` (`src/parse.rs:144`) calls
`parse_all_cached(inv, &mut HashMap::new())`. `parse_all_cached` writes each
fresh parse only into a map entry that already exists. It then builds its
result from the map, so an empty map gives back one default `FileParse` per
file, with no refs and no symbols.

The CLI is unaffected: `lib::run` calls `inventory::collect_cached` and
`parse_all_cached` with the same store, and the inventory fills the entries
first. The bug only reaches library callers that use `parse_all` directly.
Found while building the resolution measurement harness, which now uses the
`collect_cached` / `parse_all_cached` pair. Not fixed.
