//! `codearch agents`: tell agents the lookups exist.
//!
//! Job    Build the short instruction block and merge it into a file agents
//!        already read (`AGENTS.md`, `CLAUDE.md`, ...). Replaces the map's one
//!        agent-useful role, pointing at the lookups, with a few lines.
//! In     Whether a route index exists; the target file's current text.
//! Out    The block, or the file's new text.
//! Fails  A start marker without its end marker (or the reverse, or out of
//!        order): the merge refuses rather than guessing where the block ends.
//!
//! Pure functions; the CLI does the reading and writing. The wording follows
//! the eval tool descriptions (`eval/openrouter_agent.py` `IMPORTERS_TOOL`,
//! `ROUTES_TOOL`) so the shipped guidance matches what was measured.

pub const START: &str = "<!-- codearch:start -->";
pub const END: &str = "<!-- codearch:end -->";

/// The block, `\n` line endings, no trailing newline. The `callers` line only
/// appears when the repository has a route index to serve it.
pub fn block(has_routes: bool) -> String {
    let mut lines = vec![
        START,
        "## Code lookups (codearch)",
        "",
        "This repository is indexed by codearch. Use these lookups instead of guessing from search:",
        "",
        "- `codearch importers <file>`: every file that imports `<file>`, directly (depth 1) or \
transitively, from a static import index. Run it before changing a file to see what else is affected.",
    ];
    if has_routes {
        lines.push(
            "- `codearch callers <View>`: frontend files requesting that backend view's routes, each with its direct importers.",
        );
    }
    lines.extend([
        "",
        "Answers come from the last `codearch` run; if files moved since, run `codearch` first.",
        END,
    ]);
    lines.join("\n")
}

/// The file's new text with `block` in place. `existing` is `None` when the
/// file does not exist yet. Idempotent: merging the same block twice yields
/// identical text. The file's line ending (CRLF or LF) is kept.
pub fn merge(existing: Option<&str>, block: &str) -> Result<String, String> {
    let Some(text) = existing else {
        return Ok(format!("{block}\n"));
    };
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let block = block.replace('\n', eol);
    let starts = text.matches(START).count();
    let ends = text.matches(END).count();
    match (starts, ends) {
        (0, 0) => {
            if text.is_empty() {
                return Ok(format!("{block}{eol}"));
            }
            let sep = if text.ends_with('\n') { eol.to_string() } else { format!("{eol}{eol}") };
            Ok(format!("{text}{sep}{block}{eol}"))
        }
        (1, 1) => {
            let start = text.find(START).expect("counted");
            let end = text.find(END).expect("counted");
            if end < start {
                return Err(format!("{END} appears before {START}; fix the file by hand"));
            }
            Ok(format!("{}{block}{}", &text[..start], &text[end + END.len()..]))
        }
        _ => Err(format!(
            "found {starts} `{START}` and {ends} `{END}` markers; expected one of each \
or none. Fix the file by hand; not guessing where the block ends"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callers_line_only_with_a_route_index() {
        assert!(block(true).contains("codearch callers"));
        assert!(!block(false).contains("codearch callers"));
        assert!(block(false).starts_with(START) && block(false).ends_with(END));
    }

    #[test]
    fn block_stays_short() {
        let words = block(true).split_whitespace().count();
        assert!(words <= 90, "{words} words");
    }

    #[test]
    fn creates_a_missing_file() {
        let b = block(false);
        assert_eq!(merge(None, &b).unwrap(), format!("{b}\n"));
    }

    #[test]
    fn appends_after_other_content_and_leaves_it_untouched() {
        let b = block(false);
        let out = merge(Some("# Notes\nKeep me.\n"), &b).unwrap();
        assert_eq!(out, format!("# Notes\nKeep me.\n\n{b}\n"));
        let no_newline = merge(Some("Keep me."), &b).unwrap();
        assert_eq!(no_newline, format!("Keep me.\n\n{b}\n"));
    }

    #[test]
    fn replaces_only_its_own_block_and_is_idempotent() {
        let old = format!("before\n\n{START}\nold text\n{END}\n\nafter\n");
        let b = block(true);
        let once = merge(Some(&old), &b).unwrap();
        assert_eq!(once, format!("before\n\n{b}\n\nafter\n"));
        assert_eq!(merge(Some(&once), &b).unwrap(), once);
    }

    #[test]
    fn keeps_crlf_line_endings() {
        let b = block(false);
        let out = merge(Some("# Notes\r\nKeep me.\r\n"), &b).unwrap();
        assert!(out.starts_with("# Notes\r\nKeep me.\r\n\r\n"));
        assert!(!out.replace("\r\n", "").contains('\n'), "no bare LF introduced");
        assert_eq!(merge(Some(&out), &b).unwrap(), out);
    }

    #[test]
    fn malformed_markers_are_refused() {
        let b = block(false);
        assert!(merge(Some(&format!("{START}\nno end\n")), &b).is_err());
        assert!(merge(Some(&format!("no start\n{END}\n")), &b).is_err());
        assert!(merge(Some(&format!("{END}\nx\n{START}\n")), &b).is_err());
        assert!(merge(Some(&format!("{START}\n{END}\n{START}\n{END}\n")), &b).is_err());
    }
}
