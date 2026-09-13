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
    // Python. Methods are `function_definition` nested in a class body; the
    // walker does not track nesting, so all read as Function.
    ("function_definition", SymbolKind::Function),
    ("class_definition", SymbolKind::Class),
];

/// Python constructors, filtered like TS `constructor`: a key-symbols list
/// full of `__init__` says nothing about any subsystem.
const PY_SKIPPED_NAMES: &[&str] = &["__init__"];

/// Marker prefix for Django `include()` string references. Carries the edge
/// kind so stage 3 needs no profile to resolve it, and so the M3
/// package-heuristic probe never sees the bare dotted path.
pub const DJANGO_INCLUDE_MARKER: &str = "django-include:";

const EXPORT_NODE: &str = "export_statement";

/// Guards against pathological nesting; real code never approaches this.
const MAX_DEPTH: usize = 400;

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileParse {
    pub file: FileId,
    pub symbols: Vec<Symbol>,
    pub refs: Vec<RawRef>,
    /// URL-shaped contract evidence for the cross-language join (slice 2):
    /// normalized paths (`api/users`), sorted and deduplicated. Stage 3
    /// ignores these; `contract::join` matches them across languages.
    pub urls: Vec<String>,
    /// The grammar reported at least one ERROR node.
    pub partial: bool,
}

pub struct Parsers {
    ts: Parser,
    js: Parser,
    py: Parser,
}

impl Parsers {
    pub fn new() -> Parsers {
        let mut ts = Parser::new();
        // TSX and TS use different grammars; TSX is a superset for our purposes
        // and reads plain .ts correctly, so one parser covers both.
        let _ = ts.set_language(&tree_sitter_typescript::LANGUAGE_TSX.into());
        let mut js = Parser::new();
        let _ = js.set_language(&tree_sitter_javascript::LANGUAGE.into());
        let mut py = Parser::new();
        let _ = py.set_language(&tree_sitter_python::LANGUAGE.into());
        Parsers { ts, js, py }
    }

    fn for_language(&mut self, l: Language) -> &mut Parser {
        match l {
            Language::Ts | Language::Tsx => &mut self.ts,
            Language::Js | Language::Jsx => &mut self.js,
            Language::Python => &mut self.py,
        }
    }
}

impl Default for Parsers {
    fn default() -> Self {
        Parsers::new()
    }
}

pub fn parse_all(inv: &Inventory) -> Vec<FileParse> {
    parse_all_cached(inv, &mut std::collections::HashMap::new())
}

/// Parse with a warm file cache. A hit reuses the stored parse with the
/// *current* file id stamped on — stored ids go stale when files are added
/// or removed, and an edge pointing at the wrong file is the failure this
/// cache exists to never produce. Misses read, parse, hash and store.
pub fn parse_all_cached(
    inv: &Inventory,
    files: &mut std::collections::HashMap<String, crate::cache::StoredFile>,
) -> Vec<FileParse> {
    use crate::cache::sha_hex;
    let need: Vec<FileId> = inv
        .files
        .iter()
        .filter(|f| {
            files
                .get(&f.rel)
                .map(|e| e.parse_hash != e.hash)
                .unwrap_or(true)
        })
        .map(|f| f.id)
        .collect();

    if std::env::var_os("CODEARCH_TIME").is_some() {
        eprintln!("time parse-cache     hits={} miss={}", inv.len() - need.len(), need.len());
    }
    let fresh: Vec<(FileId, FileParse, String)> = need        .par_iter()
        .map_init(Parsers::new, |parsers, &id| {
            let f = inv.get(id);
            let text = std::fs::read_to_string(&f.abs).unwrap_or_default();
            let mut parsed = parse_one(parsers, id, f.language, &text);
            parsed.file = id;
            (id, parsed, sha_hex(text.as_bytes()))
        })
        .collect();

    for (id, parsed, hash) in fresh {
        let rel = inv.get(id).rel.clone();
        if let Some(entry) = files.get_mut(&rel) {
            entry.parse = parsed;
            entry.parse_hash = hash.clone();
            entry.hash = hash;
        }
    }

    inv.files
        .iter()
        .map(|f| {
            let mut p = files
                .get(&f.rel)
                .map(|e| e.parse.clone())
                .unwrap_or_default();
            p.file = f.id;
            p
        })
        .collect()
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
    visit(root, src, false, lang.is_python(), &mut out, 0);
    out.urls.sort();
    out.urls.dedup();
    out
}

fn visit(node: Node, src: &str, in_export: bool, py: bool, out: &mut FileParse, depth: usize) {
    if depth > MAX_DEPTH {
        out.partial = true;
        return;
    }

    let kind = node.kind();
    let line = node.start_position().row + 1;

    if py {
        visit_python(node, src, line, out);
    } else {
        let exported = in_export || kind == EXPORT_NODE;

        if IMPORT_NODES.contains(&kind) {
            if let Some(source) = node.child_by_field_name("source") {
                if let Some(spec) = string_value(source, src) {
                    out.refs.push(RawRef {
                        specifier: spec,
                        line,
                    });
                }
            }
        }

        if kind == "call_expression" {
            if let Some(spec) = import_call_specifier(node, src) {
                out.refs.push(RawRef {
                    specifier: spec,
                    line,
                });
            }
            // Contract evidence (slice 2): the first string-valued call
            // argument starting with `/`. `fetch`, axios-likes and router
            // calls all carry the path first; dynamic URLs (templates,
            // concatenation) are not string literals and stay out.
            if let Some(raw) = ts_first_string_arg(node, src) {
                if raw.starts_with('/') {
                    if let Some(u) = normalize_url(&raw) {
                        out.urls.push(u);
                    }
                }
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
                            line,
                        });
                    }
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit(child, src, in_export || kind == EXPORT_NODE, py, out, depth + 1);
    }
}

/// Python half of `visit`. Import extraction reads node text rather than
/// grammar fields: field names drift between grammar versions, while the
/// surface syntax (`import a.b`, `from .x import y`) does not.
fn visit_python(node: Node, src: &str, line: usize, out: &mut FileParse) {
    match node.kind() {
        "import_statement" => {
            for spec in python_import_names(node, src) {
                out.refs.push(RawRef { specifier: spec, line });
            }
        }
        "import_from_statement" => {
            if let Some(spec) = python_from_module(node, src) {
                out.refs.push(RawRef { specifier: spec, line });
            }
        }
        // The grammar gives `from __future__ import x` its own node kind.
        // Recorded so stage 3 can file it external instead of unresolved.
        "future_import_statement" => {
            out.refs.push(RawRef { specifier: "__future__".to_string(), line });
        }
        // Python calls are `call` with an `argument_list`, not the TS
        // `call_expression`/`arguments` shape. The callee is the first named
        // child (`identifier`, or `attribute` for `obj.method()`).
        "call" => {
            if let Some(spec) = django_include_specifier(node, src) {
                out.refs.push(RawRef { specifier: spec, line });
            }
            // Django `path('api/x/', …)`: same first-string convention as
            // decorators, gated on the callee name.
            if let Some(raw) = path_call_string(node, src) {
                if let Some(u) = normalize_url(&raw) {
                    out.urls.push(u);
                }
            }
        }
        // Route decorators (`@app.route('/api/x')`, `@bp.get('api/x')`):
        // the contract path is the first string argument in every
        // framework. No leading slash required — Django omits it.
        "decorator" => {
            if let Some(raw) = first_string_descendant(node, src) {
                if let Some(u) = normalize_url(&raw) {
                    out.urls.push(u);
                }
            }
        }
        "function_definition" | "class_definition" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                if let Ok(name) = name_node.utf8_text(src.as_bytes()) {
                    // No export keyword in Python; everything reads
                    // unexported. `__init__` is noise, as decided in M4.
                    if !PY_SKIPPED_NAMES.contains(&name) && name.len() >= 3 {
                        out.symbols.push(Symbol {
                            name: name.to_string(),
                            kind: if node.kind() == "class_definition" {
                                SymbolKind::Class
                            } else {
                                SymbolKind::Function
                            },
                            exported: false,
                            line,
                        });
                    }
                }
            }
        }
        _ => {}
    }
}

/// `import a` / `import a.b as c, d` → one dotted specifier per name.
/// Reads `dotted_name` descendants so `aliased_import` wrappers need no
/// special field knowledge.
fn python_import_names(node: Node, src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "dotted_name" {
            if let Ok(text) = n.utf8_text(src.as_bytes()) {
                if is_dotted_path(text) {
                    out.push(text.to_string());
                }
            }
            continue;
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            if child.is_named() {
                stack.push(child);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `from X import y` → the module slice between the keywords: `.model`,
/// `..pkg`, `flask`, or `.` for bare `from . import y`. Slicing source text
/// instead of reading grammar fields keeps this working across grammar
/// versions; the shape check rejects anything that is not a module path.
fn python_from_module(node: Node, src: &str) -> Option<String> {
    let bytes = src.as_bytes();
    let mut cursor = node.walk();
    let mut from_end = None;
    let mut import_start = None;
    for child in node.children(&mut cursor) {
        if child.is_named() {
            continue;
        }
        let Ok(text) = child.utf8_text(bytes) else {
            continue;
        };
        if text == "from" && from_end.is_none() {
            from_end = Some(child.end_byte());
        } else if text == "import" {
            import_start = Some(child.start_byte());
            break;
        }
    }
    let slice = src.get(from_end?..import_start?)?.trim();
    // Parenthesized continuation or line break inside: not a static fact.
    if slice.is_empty() || slice.contains(['(', ')', '\n', '\\']) {
        return None;
    }
    let dots = slice.len() - slice.trim_start_matches('.').len();
    let rest = &slice[dots..];
    if rest.is_empty() {
        // `from . import x` / `from .. import x`: the package itself.
        return Some(".".repeat(dots.max(1)));
    }
    if !is_dotted_path(rest) {
        return None;
    }
    Some(slice.to_string())
}

/// Django URL wiring: `include('conduit.apps.articles.urls')` is a real
/// dependency with no import statement behind it. Only the first-arg string
/// that parses as a dotted path is taken; URL patterns, `namespace=`
/// kwargs and non-string args yield nothing.
fn django_include_specifier(node: Node, src: &str) -> Option<String> {
    let bytes = src.as_bytes();
    let mut cursor = node.walk();
    let kids: Vec<_> = node.children(&mut cursor).collect();
    let callee = kids.iter().find(|c| c.is_named())?;
    if callee.utf8_text(bytes).ok()? != "include" {
        return None;
    }
    let args = kids.iter().find(|c| c.kind() == "argument_list")?;
    let mut cursor = args.walk();
    let first = args.named_children(&mut cursor).next()?;
    let text = string_value(first, src)?;
    if !is_dotted_path(&text) {
        return None;
    }
    Some(format!("{DJANGO_INCLUDE_MARKER}{text}"))
}

fn is_dotted_path(s: &str) -> bool {
    let mut parts = s.split('.');
    match parts.next() {
        Some(first) if is_identifier(first) => {}
        _ => return false,
    }
    parts.all(is_identifier)
}

fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
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
/// a computed specifier is not a static fact. Handles both the TS
/// `string_fragment` child and the Python `string_content` one.
fn string_value(node: Node, src: &str) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_fragment" || child.kind() == "string_content" {
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

/// Normalize a URL-ish string to a joinable contract path, or `None` when
/// the string is not URL-shaped. Leading/trailing slashes, query strings and
/// fragments go; dynamic segments (`:id`, `<int:id>`, pure digits) collapse
/// to `{}` so a concrete fetch matches its parameterized route. One surviving
/// segment (`/health`) is not a contract — too collision-prone to join on.
pub fn normalize_url(raw: &str) -> Option<String> {
    let bare = raw.split(['?', '#']).next().unwrap_or(raw);
    let mut segs: Vec<&str> = Vec::new();
    for seg in bare.split('/') {
        if seg.is_empty() {
            continue;
        }
        if seg.starts_with(':')
            || (seg.starts_with('<') && seg.ends_with('>'))
            || seg.bytes().all(|b| b.is_ascii_digit())
        {
            segs.push("{}");
        } else {
            segs.push(seg);
        }
    }
    if segs.len() < 2 {
        return None;
    }
    Some(segs.join("/"))
}

/// First string-valued argument of a TS call, in source order.
fn ts_first_string_arg(node: Node, src: &str) -> Option<String> {
    let args = node.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    for child in args.named_children(&mut cursor) {
        if child.kind() == "string" {
            if let Some(s) = string_value(child, src) {
                return Some(s);
            }
        }
    }
    None
}

/// First string literal under a node, in source order. The callee of a call
/// is an identifier or attribute, never a string, so over a whole call node
/// this is the first string argument — which is the route path by convention
/// in every framework's decorators and `path()` calls.
fn first_string_descendant(node: Node, src: &str) -> Option<String> {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "string" {
            if let Some(s) = string_value(n, src) {
                return Some(s);
            }
            continue;
        }
        let mut cursor = n.walk();
        let kids: Vec<_> = n
            .children(&mut cursor)
            .filter(|c| c.is_named())
            .collect();
        for k in kids.into_iter().rev() {
            stack.push(k);
        }
    }
    None
}

/// `path('api/x/', …)` / `re_path` / `url`: Django URLconfs without
/// `include()`. Gated on the callee name so ordinary calls never contribute.
fn path_call_string(node: Node, src: &str) -> Option<String> {
    let mut cursor = node.walk();
    let first = node.children(&mut cursor).find(|c| c.is_named())?;
    let callee = match first.kind() {
        "identifier" => first.utf8_text(src.as_bytes()).ok()?.to_string(),
        "attribute" => first
            .utf8_text(src.as_bytes())
            .ok()?
            .rsplit('.')
            .next()?
            .to_string(),
        _ => return None,
    };
    if !matches!(callee.as_str(), "path" | "re_path" | "url") {
        return None;
    }
    first_string_descendant(node, src)
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
    fn broken_source_is_partial_not_fatal() {        let out = parse("import { A } from './a'; function ( { ", Language::Ts);
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

    fn parse_py(src: &str) -> FileParse {
        parse(src, Language::Python)
    }

    #[test]
    fn python_import_forms() {
        let out = parse_py(
            "import os\nimport sqlalchemy.orm as sa_orm\nfrom .model import Model\nfrom ..pkg import Thing\nfrom . import Entry\nfrom flask import Flask\n",
        );
        let mut specs: Vec<&str> = out.refs.iter().map(|r| r.specifier.as_str()).collect();
        specs.sort();
        assert_eq!(
            specs,
            vec![".", "..pkg", ".model", "flask", "os", "sqlalchemy.orm"]
        );
    }

    #[test]
    fn python_future_import_is_kept_for_resolve() {
        let out = parse_py("from __future__ import annotations\n");
        assert_eq!(
            out.refs.iter().map(|r| r.specifier.as_str()).collect::<Vec<_>>(),
            vec!["__future__"]
        );
    }

    #[test]
    fn python_symbols_skip_init() {
        let out = parse_py("class Article:\n    def __init__(self): ...\n    def publish(self): ...\n");
        let names: Vec<&str> = out.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Article"));
        assert!(names.contains(&"publish"));
        assert!(!names.contains(&"__init__"));
        assert!(out.symbols.iter().all(|s| !s.exported));
    }

    #[test]
    fn python_include_strings_are_marked() {
        let out = parse_py("urlpatterns = [url(r'^api/', include('conduit.apps.articles.urls'))]\n");
        assert_eq!(
            out.refs.iter().map(|r| r.specifier.as_str()).collect::<Vec<_>>(),
            vec!["django-include:conduit.apps.articles.urls"]
        );
    }

    #[test]
    fn python_include_rejects_patterns_and_other_callees() {
        let out = parse_py("x = include(router.urls)\ny = path('a', view)\nz = include(r'^api/')\n");
        assert!(out.refs.is_empty());
    }

    #[test]
    fn python_broken_source_is_partial() {
        let out = parse_py("from .model import Model\ndef broken(:\n");
        assert!(out.partial);
        assert!(out.refs.iter().any(|r| r.specifier == ".model"));
    }
}
