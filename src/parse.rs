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
    /// Python route declarations (`path`/`re_path`/`url`/`router.register`)
    /// with their `include()` prefixes, for `contract::route_join`.
    #[serde(default)]
    pub routes: Vec<Route>,
    /// TS: route-shaped segments of short, space-free string and template
    /// literals (`'tags'`, `` `${base}documents/bulk_edit/` ``), sorted.
    #[serde(default)]
    pub url_segments: Vec<String>,
    /// TS: the file text names an HTTP client (`HttpClient`, `this.http.`,
    /// `fetch(`, `axios`, `apiBaseUrl`).
    #[serde(default)]
    pub http: bool,
    /// TS: base classes named in `extends` clauses, generics stripped.
    #[serde(default)]
    pub extends: Vec<String>,
    /// The grammar reported at least one ERROR node.
    pub partial: bool,
}

/// One backend route: static path segments (include prefixes first) bound
/// to the view named in the same call (`TagViewSet`, `Upload.as_view()`).
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Route {
    pub segments: Vec<String>,
    pub view: String,
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
    visit(root, src, false, lang.is_python(), &mut out, 0, false);    out.urls.sort();
    out.urls.dedup();
    if lang.is_python() {
        python_routes(root, src, &[], &mut out.routes, 0);
    } else {
        out.http = HTTP_MARKERS.iter().any(|m| src.contains(m));
        out.url_segments.sort();
        out.url_segments.dedup();
    }
    out
}

/// Text evidence that a TS file sends HTTP requests itself.
const HTTP_MARKERS: &[&str] = &["HttpClient", "this.http.", "fetch(", "axios", "apiBaseUrl"];

/// Object keys whose values are client-side route paths (Angular and React
/// router config), never request URLs.
const ROUTER_PATH_KEYS: &[&str] = &["path", "redirectTo", "routerLink"];

/// Method names whose string arguments name something other than a request
/// URL: client-side navigation, DOM lookups, form controls. `get` is absent
/// on purpose: `form.get('name')` reads a control but `http.get(url)` sends
/// a request, so it is decided per receiver (see `call_args_are_non_request`).
const NON_REQUEST_METHODS: &[&str] = &[
    "navigate",
    "navigateByUrl",
    "getElementById",
    "querySelector",
    "addControl",
    "removeControl",
    "setControl",
];

/// Longest literal read for route segments. Route strings are short; long
/// literals are prose, markup or data.
const MAX_ROUTE_LITERAL: usize = 120;

/// Static segments of a route pattern or request URL. Parameters of every
/// shape — `<int:pk>`, `(?P<pk>[^/]+)`, `${id}`, `[^/]+`, `\d+` — become
/// separators, so `^documents/(?P<pk>\d+)/notes/$` and
/// `` `${base}documents/${id}/notes/` `` both yield `documents`, `notes`.
/// Segments start with a letter and are at least two characters.
pub fn route_segments(raw: &str) -> Vec<String> {
    fn flush(cur: &mut String, out: &mut Vec<String>) {
        if cur.len() >= 2 && cur.starts_with(|c: char| c.is_ascii_alphabetic()) {
            out.push(std::mem::take(cur));
        }
        cur.clear();
    }
    let chars: Vec<char> = raw.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let close = match c {
            '(' => Some(')'),
            '<' => Some('>'),
            '[' => Some(']'),
            '{' => Some('}'),
            _ => None,
        };
        if let Some(close) = close {
            flush(&mut cur, &mut out);
            let mut depth = 0;
            while i < chars.len() {
                if chars[i] == c {
                    depth += 1;
                } else if chars[i] == close {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                i += 1;
            }
        } else if c == '\\' {
            flush(&mut cur, &mut out);
            i += 1; // the escaped character is regex syntax, not a segment
        } else if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            cur.push(c);
        } else {
            flush(&mut cur, &mut out);
        }
        i += 1;
    }
    flush(&mut cur, &mut out);
    out
}

/// Django route declarations under `node`, with `include()` nesting as a
/// segment prefix. A call named `path`/`re_path`/`url`/`register` whose first
/// argument is a string is a route: its second argument names the view
/// (`View`, `View.as_view()`, `module.view`) or holds an `include(...)`,
/// whose routes inherit the pattern as prefix.
fn python_routes(node: Node, src: &str, prefix: &[String], out: &mut Vec<Route>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    if node.kind() == "call" {
        if let Some((pattern, target)) = route_call(node, src) {
            let mut segments = prefix.to_vec();
            segments.extend(route_segments(&pattern));
            if let Some(view) = view_name(target, src) {
                out.push(Route { segments, view });
            } else {
                python_routes(target, src, &segments, out, depth + 1);
            }
            return;
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        python_routes(child, src, prefix, out, depth + 1);
    }
}

/// `(pattern, second positional argument)` of a route-declaring call.
fn route_call<'a>(node: Node<'a>, src: &str) -> Option<(String, Node<'a>)> {
    let callee = node.child_by_field_name("function")?;
    let name = callee.utf8_text(src.as_bytes()).ok()?.rsplit('.').next()?;
    if !matches!(name, "path" | "re_path" | "url" | "register") {
        return None;
    }
    let args = node.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    let mut positional = args
        .named_children(&mut cursor)
        .filter(|c| c.kind() != "keyword_argument" && c.kind() != "comment");
    let pattern = string_value(positional.next()?, src)?;
    Some((pattern, positional.next()?))
}

/// The view a route argument names: `View`, `View.as_view(...)` or
/// `module.view`. `None` for anything else (an `include(...)`, a list).
fn view_name(node: Node, src: &str) -> Option<String> {
    let text = |n: Node| n.utf8_text(src.as_bytes()).ok().map(str::to_string);
    match node.kind() {
        "identifier" => text(node),
        "attribute" => text(node)?.rsplit('.').next().map(str::to_string),
        "call" => {
            let callee = node.child_by_field_name("function")?;
            let t = text(callee)?;
            let base = t.strip_suffix(".as_view")?;
            is_dotted_path(base).then(|| base.rsplit('.').next().unwrap_or(base).to_string())
        }
        _ => None,
    }
}

fn visit(
    node: Node,
    src: &str,
    in_export: bool,
    py: bool,
    out: &mut FileParse,
    depth: usize,
    in_specifier: bool,
) {
    if depth > MAX_DEPTH {
        out.partial = true;
        return;
    }

    let kind = node.kind();
    let line = node.start_position().row + 1;

    // Strings in non-request positions name files, keys or client-side
    // paths, not URLs. The walker has no parent pointer, so one flag is
    // threaded down: each construct below marks the child holding its
    // non-URL text, and every literal underneath stays out of
    // `url_segments`.
    //   import/export `source`, `require()`/`import()` arguments: files.
    //   subscript `index` (`data['documents']`), indexed-access
    //   `literal_type` (`T['status']`): keys.
    //   client-side router arguments (`.navigate([...])`,
    //   `.navigateByUrl(...)`, `path:`/`redirectTo:`/`routerLink:` values):
    //   browser routes.
    //   form and DOM accessors (`.get(`, `.getElementById(`, ...): control
    //   and element names — except `http.get(url)`, which is a request.
    let is_source = IMPORT_NODES.contains(&kind) && node.child_by_field_name("source").is_some();
    let is_import_call =
        kind == "call_expression" && import_call_specifier(node, src).is_some();
    let skip_args = kind == "call_expression" && call_args_are_non_request(node, src);
    let router_value = router_path_value(node, src);
    let subscript_index = (kind == "subscript_expression")
        .then(|| node.child_by_field_name("index"))
        .flatten();
    // `T['status']`: the string sits in a `literal_type` under a
    // `lookup_type`. Type-level text is never a request URL.
    let in_type_literal = kind == "literal_type";

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

        // Route evidence: short, space-free literals only; UI text and
        // markup have spaces or length, route strings do not. Literals in
        // non-request positions (flagged above) stay out.
        let literal = match kind {
            "string" if !in_specifier => string_value(node, src),
            "template_string" if !in_specifier => {
                node.utf8_text(src.as_bytes()).ok().map(str::to_string)
            }
            _ => None,
        };
        if let Some(lit) = literal {
            if lit.len() <= MAX_ROUTE_LITERAL && !lit.contains(char::is_whitespace) {
                out.url_segments.extend(route_segments(&lit));
            }
        }
        if kind == "extends_clause" {
            if let Some(value) = node.child_by_field_name("value") {
                if let Ok(t) = value.utf8_text(src.as_bytes()) {
                    let base = t.split('<').next().unwrap_or(t).trim();
                    if is_identifier(base) {
                        out.extends.push(base.to_string());
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
    let source = node.child_by_field_name("source");
    let arguments = node.child_by_field_name("arguments");
    for child in node.children(&mut cursor) {
        let child_spec = in_specifier
            || (is_source && source.is_some_and(|s| s.id() == child.id()))
            || ((is_import_call || skip_args)
                && arguments.is_some_and(|a| a.id() == child.id()))
            || subscript_index.is_some_and(|ix| ix.id() == child.id())
            || router_value.is_some_and(|v| v.id() == child.id())
            || in_type_literal;
        visit(
            child,
            src,
            in_export || kind == EXPORT_NODE,
            py,
            out,
            depth + 1,
            child_spec,
        );
    }
}

/// Callee `object.method` of a call, when member-shaped.
fn call_method(node: Node, src: &str) -> Option<(String, String)> {
    let callee = node.child_by_field_name("function")?;
    if callee.kind() != "member_expression" {
        return None;
    }
    let object = callee.child_by_field_name("object")?;
    let property = callee.child_by_field_name("property")?;
    Some((
        object.utf8_text(src.as_bytes()).ok()?.to_string(),
        property.utf8_text(src.as_bytes()).ok()?.to_string(),
    ))
}

/// Whether a call's `arguments` subtree holds no request URL: client-side
/// navigation, DOM lookups, form controls. `get` is decided per receiver:
/// `form.get('name')` reads a control, `http.get(url)` sends a request.
fn call_args_are_non_request(node: Node, src: &str) -> bool {
    let Some((object, method)) = call_method(node, src) else {
        return false;
    };
    if NON_REQUEST_METHODS.contains(&method.as_str()) {
        return true;
    }
    method == "get" && !object.contains("http")
}

/// The `value` child of a `{ path: ... }`-shaped pair whose key names a
/// client-side route, or `None`. Keys may be identifiers or quoted strings.
fn router_path_value<'a>(node: Node<'a>, src: &str) -> Option<Node<'a>> {
    if node.kind() != "pair" {
        return None;
    }
    let key = node.child_by_field_name("key")?;
    let text = key.utf8_text(src.as_bytes()).ok()?;
    let name = text.trim_matches(|c| c == '\'' || c == '"');
    ROUTER_PATH_KEYS
        .contains(&name)
        .then(|| node.child_by_field_name("value"))
        .flatten()
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

    #[test]
    fn route_segments_drop_parameters_of_every_shape() {
        assert_eq!(route_segments("^documents/(?P<pk>[^/.]+)/notes/$"), vec!["documents", "notes"]);
        assert_eq!(route_segments("users/<int:pk>/profile/"), vec!["users", "profile"]);
        assert_eq!(route_segments("`${this.baseUrl}${this.resourceName}/bulk_edit/`"), vec!["bulk_edit"]);
        assert_eq!(route_segments("share_links"), vec!["share_links"]);
        assert!(route_segments("/1/x/").is_empty());
    }

    #[test]
    fn django_routes_carry_include_prefixes_and_view_names() {
        let out = parse_py(
            "router.register(r'tags', TagViewSet)
urlpatterns = [
    re_path(r'^api/', include([
        re_path('^documents/', include([
            re_path('^bulk_edit/', BulkEditView.as_view(), name='bulk_edit'),
        ])),
        path('profile/', views.ProfileView.as_view()),
        path('login/', allauth_views.login),
    ])),
    path('admin/', admin.site.urls),
]
",
        );
        let got: Vec<(Vec<String>, String)> =
            out.routes.iter().map(|r| (r.segments.clone(), r.view.clone())).collect();
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(got.contains(&(s(&["tags"]), "TagViewSet".into())));
        assert!(got.contains(&(s(&["api", "documents", "bulk_edit"]), "BulkEditView".into())));
        assert!(got.contains(&(s(&["api", "profile"]), "ProfileView".into())));
        assert!(got.contains(&(s(&["api", "login"]), "login".into())));
    }

    #[test]
    fn ts_route_evidence_reads_literals_http_and_extends() {
        let out = parse(
            "export class TagService extends AbstractNameFilterService<Tag> {
  constructor() { super(); this.resourceName = 'tags' }
  label = 'Save changes now'
}
",
            Language::Ts,
        );
        assert_eq!(out.extends, vec!["AbstractNameFilterService"]);
        assert!(!out.http);
        assert!(out.url_segments.contains(&"tags".to_string()));
        assert!(!out.url_segments.contains(&"Save".to_string()), "prose is not route evidence");
        let base = parse(
            "export abstract class AbstractPaperlessService { url = `${environment.apiBaseUrl}x/` }
",
            Language::Ts,
        );
        assert!(base.http);
    }

    #[test]
    fn import_specifiers_are_not_route_evidence() {
        // `tags` in a module path is a file name, not a request URL.
        let out = parse(
            "import { X } from '../common/tags/tags.component'\n",
            Language::Ts,
        );
        assert!(!out.url_segments.contains(&"tags".to_string()));
        assert!(out.refs.iter().any(|r| r.specifier.contains("tags")));
        // The same word as a resource name is route evidence again.
        let back = parse(
            "import { X } from '../common/tags/tags.component'\nresourceName = 'tags'\n",
            Language::Ts,
        );
        assert!(back.url_segments.contains(&"tags".to_string()));
        // `require()` / `import()` specifiers stay out too.
        let req = parse("const x = require('../common/tags/tags.component')\n", Language::Js);
        assert!(!req.url_segments.contains(&"tags".to_string()));
        let dyn_import = parse(
            "async function load() { return import('../common/tags/tags.component') }\n",
            Language::Js,
        );
        assert!(!dyn_import.url_segments.contains(&"tags".to_string()));
    }

    #[test]
    fn subscript_keys_are_not_route_evidence() {
        // `data['documents'] = documents` names an object key, not a URL.
        let out = parse("data['documents'] = documents\n", Language::Ts);
        assert!(!out.url_segments.contains(&"documents".to_string()));
        // Type-level indexed access is a key too: `T['status']`.
        let ty = parse("function f(x: T['status']) {}\n", Language::Ts);
        assert!(!ty.url_segments.contains(&"status".to_string()));
        // A request template still counts.
        let url = parse("fetch(`${base}documents/1/`)\n", Language::Ts);
        assert!(url.url_segments.contains(&"documents".to_string()));
    }

    #[test]
    fn client_side_router_paths_are_not_route_evidence() {
        let out = parse("this.router.navigate(['documents', id])\n", Language::Ts);
        assert!(!out.url_segments.contains(&"documents".to_string()));
        let by_url = parse("this.router.navigateByUrl('/documents')\n", Language::Ts);
        assert!(!by_url.url_segments.contains(&"documents".to_string()));
        let cfg = parse("const r = { path: 'documents', redirectTo: 'trash' }\n", Language::Ts);
        assert!(!cfg.url_segments.contains(&"documents".to_string()));
        assert!(!cfg.url_segments.contains(&"trash".to_string()));
        // Other object values are untouched.
        let other = parse("const r = { endpoint: 'documents' }\n", Language::Ts);
        assert!(other.url_segments.contains(&"documents".to_string()));
    }

    #[test]
    fn form_and_dom_accessors_are_not_route_evidence_but_http_get_is() {
        // `form.get('custom_fields')` reads a control.
        let form = parse("this.documentForm.get('custom_fields')\n", Language::Ts);
        assert!(!form.url_segments.contains(&"custom_fields".to_string()));
        let dom = parse("document.getElementById('documents')\n", Language::Ts);
        assert!(!dom.url_segments.contains(&"documents".to_string()));
        // `http.get(url)` sends a request: the URL template still counts.
        let http = parse("this.http.get(`${base}documents/1/`)\n", Language::Ts);
        assert!(http.url_segments.contains(&"documents".to_string()));
        assert!(http.http);
    }
}
