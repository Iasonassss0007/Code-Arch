//! Stage 2 — Parse.
//!
//! Job    Extract syntax-level facts per file.
//! In     Inventory
//! Out    Vec<FileParse> { symbols, refs, partial }
//! Fails  Parse error -> tree-sitter yields a partial tree; usable nodes are kept
//!        and the file is flagged `partial`. Never fatal.
//!
//! This stage reports only that a specifier was *written*. Deciding which file
//! it resolves to is stage 3's job, and conflating the two is the usual way a
//! tool like this becomes quietly wrong.
//!
//! Node kinds live in a declarative table rather than `.scm` queries: a query
//! naming a node kind absent from a given grammar version fails to compile
//! outright, whereas an unknown entry in the table simply never matches.

use crate::inventory::Inventory;
use crate::types::{FileId, Language, RawRef, Symbol, SymbolKind};
use rayon::prelude::*;
use tree_sitter::{Node, Parser};

/// Node kinds that carry a module specifier in their `source` field.
const IMPORT_NODES: &[&str] = &["import_statement", "export_statement"];

/// Callee names that take a module specifier as their first argument.
const IMPORT_CALLS: &[&str] = &["require", "import"];

/// Declaration node kind -> symbol kind. The `name` field supplies the name.
/// Kinds absent from a grammar are simply never encountered.
const SYMBOL_NODES: &[(&str, SymbolKind)] = &[
    ("function_declaration", SymbolKind::Function),
    ("generator_function_declaration", SymbolKind::Function),
    ("function_signature", SymbolKind::Function),
    ("class_declaration", SymbolKind::Class),
    ("abstract_class_declaration", SymbolKind::Class),
    ("interface_declaration", SymbolKind::Interface),
    ("type_alias_declaration", SymbolKind::TypeAlias),
    ("enum_declaration", SymbolKind::Enum),
    ("method_definition", SymbolKind::Method),
    ("method_signature", SymbolKind::Method),
    ("variable_declarator", SymbolKind::Const),
];

const EXPORT_NODE: &str = "export_statement";

/// Guards against pathological nesting; real code never approaches this.
const MAX_DEPTH: usize = 400;

#[derive(Debug, Default, Clone)]
pub struct FileParse {
    pub file: FileId,
    pub symbols: Vec<Symbol>,
    pub refs: Vec<RawRef>,
    /// The grammar reported at least one ERROR node.
    pub partial: bool,
}

pub struct Parsers {
    ts: Parser,
    js: Parser,
}

impl Parsers {
    pub fn new() -> Parsers {
        let mut ts = Parser::new();
        // TSX and TS use different grammars; TSX is a superset for our purposes
        // and reads plain .ts correctly, so one parser covers both.
        let _ = ts.set_language(&tree_sitter_typescript::LANGUAGE_TSX.into());
        let mut js = Parser::new();
        let _ = js.set_language(&tree_sitter_javascript::LANGUAGE.into());
        Parsers { ts, js }
    }

    fn for_language(&mut self, l: Language) -> &mut Parser {
        match l {
            Language::Ts | Language::Tsx => &mut self.ts,
            Language::Js | Language::Jsx => &mut self.js,
        }
    }
}

impl Default for Parsers {
    fn default() -> Self {
        Parsers::new()
    }
}

pub fn parse_all(inv: &Inventory) -> Vec<FileParse> {
    let mut out: Vec<FileParse> = inv
        .files
        .par_iter()
        .map_init(Parsers::new, |parsers, f| {
            let text = std::fs::read_to_string(&f.abs).unwrap_or_default();
            parse_one(parsers, f.id, f.language, &text)
        })
        .collect();
    out.sort_by_key(|p| p.file);
    out
}

pub fn parse_one(parsers: &mut Parsers, id: FileId, lang: Language, src: &str) -> FileParse {
    let mut out = FileParse {
        file: id,
        ..Default::default()
    };
    if src.is_empty() {
        return out;
    }
    let Some(tree) = parsers.for_language(lang).parse(src, None) else {
        out.partial = true;
        return out;
    };
    let root = tree.root_node();
    out.partial = root.has_error();
    visit(root, src, false, &mut out, 0);
    out
}

fn visit(node: Node, src: &str, in_export: bool, out: &mut FileParse, depth: usize) {
    if depth > MAX_DEPTH {
        out.partial = true;
        return;
    }

    let kind = node.kind();
    let exported = in_export || kind == EXPORT_NODE;

    if IMPORT_NODES.contains(&kind) {
        if let Some(source) = node.child_by_field_name("source") {
            if let Some(spec) = string_value(source, src) {
                out.refs.push(RawRef {
                    specifier: spec,
                    line: node.start_position().row + 1,
                });
            }
        }
    }

    if kind == "call_expression" {
        if let Some(spec) = import_call_specifier(node, src) {
            out.refs.push(RawRef {
                specifier: spec,
                line: node.start_position().row + 1,
            });
        }
    }

    if let Some((_, sym_kind)) = SYMBOL_NODES.iter().find(|(k, _)| *k == kind) {
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Ok(name) = name_node.utf8_text(src.as_bytes()) {
                if is_interesting_symbol(name, *sym_kind) {
                    out.symbols.push(Symbol {
                        name: name.to_string(),
                        kind: *sym_kind,
                        exported,
                        line: node.start_position().row + 1,
                    });
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit(child, src, exported, out, depth + 1);
    }
}

/// `require("x")` and `import("x")` both take the specifier as first argument.
fn import_call_specifier(node: Node, src: &str) -> Option<String> {
    let callee = node.child_by_field_name("function")?;
    let callee_text = callee.utf8_text(src.as_bytes()).ok()?;
    if !IMPORT_CALLS.contains(&callee_text) {
        return None;
    }
    let args = node.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    let first = args.named_children(&mut cursor).next()?;
    string_value(first, src)
}

/// Text of a string literal without its quotes. Template strings are skipped:
/// a computed specifier is not a static fact.
fn string_value(node: Node, src: &str) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_fragment" {
            return child.utf8_text(src.as_bytes()).ok().map(|s| s.to_string());
        }
    }
    // Empty string literal.
    let raw = node.utf8_text(src.as_bytes()).ok()?;
    if raw.len() >= 2 {
        Some(raw[1..raw.len() - 1].to_string())
    } else {
        None
    }
}

/// Filters the noise that `variable_declarator` would otherwise flood in with.
fn is_interesting_symbol(name: &str, kind: SymbolKind) -> bool {
    if name.len() < 3 {
        return false;
    }
    match kind {
        // Only keep bindings that look like a declared thing, not loop locals.
        SymbolKind::Const => {
            let first = name.chars().next().unwrap_or('a');
            first.is_uppercase() || name.len() >= 6
        }
        SymbolKind::Method => !matches!(name, "constructor" | "toString" | "valueOf"),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str, lang: Language) -> FileParse {
        let mut p = Parsers::new();
        parse_one(&mut p, 0, lang, src)
    }

    #[test]
    fn extracts_static_imports_and_reexports() {
        let out = parse(
            r#"
            import { UserRepository } from './users';
            import type { A } from "@/types";
            export { b } from './b';
            "#,
            Language::Ts,
        );
        let specs: Vec<&str> = out.refs.iter().map(|r| r.specifier.as_str()).collect();
        assert!(specs.contains(&"./users"));
        assert!(specs.contains(&"@/types"));
        assert!(specs.contains(&"./b"));
    }

    #[test]
    fn extracts_require_and_dynamic_import() {
        let out = parse(
            r#"
            const fs = require('node:fs');
            async function load() { return import('./lazy'); }
            const nope = notRequire('./skip');
            "#,
            Language::Js,
        );
        let specs: Vec<&str> = out.refs.iter().map(|r| r.specifier.as_str()).collect();
        assert!(specs.contains(&"node:fs"));
        assert!(specs.contains(&"./lazy"));
        assert!(!specs.contains(&"./skip"));
    }

    #[test]
    fn extracts_declarations_with_export_flag() {
        let out = parse(
            r#"
            export function authenticateUser() {}
            class SessionStore {}
            export interface Credentials { id: string }
            "#,
            Language::Ts,
        );
        let find = |n: &str| out.symbols.iter().find(|s| s.name == n).cloned();
        let auth = find("authenticateUser").expect("function captured");
        assert_eq!(auth.kind, SymbolKind::Function);
        assert!(auth.exported);
        let store = find("SessionStore").expect("class captured");
        assert!(!store.exported);
        assert_eq!(find("Credentials").unwrap().kind, SymbolKind::Interface);
    }

    #[test]
    fn broken_source_is_partial_not_fatal() {
        let out = parse("import { A } from './a'; function ( { ", Language::Ts);
        assert!(out.partial);
        // The salvageable part is still extracted.
        assert_eq!(out.refs.len(), 1);
    }

    #[test]
    fn tsx_generics_do_not_break_extraction() {
        let out = parse(
            "export const Card = <T,>(p: T) => <div>{String(p)}</div>;",
            Language::Tsx,
        );
        assert!(out.symbols.iter().any(|s| s.name == "Card"));
    }
}
