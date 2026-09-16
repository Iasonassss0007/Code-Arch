//! Shared types across pipeline stages.

use std::path::{Path, PathBuf};

pub type FileId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum Language {
    #[default]
    Ts,
    Tsx,
    Js,
    Jsx,
    Python,
}

impl Language {
    pub fn from_path(p: &Path) -> Option<Language> {
        match p.extension().and_then(|e| e.to_str())? {
            "ts" | "mts" | "cts" => Some(Language::Ts),
            "tsx" => Some(Language::Tsx),
            "js" | "mjs" | "cjs" => Some(Language::Js),
            "jsx" => Some(Language::Jsx),
            "py" => Some(Language::Python),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Language::Ts => "TypeScript",
            Language::Tsx => "TSX",
            Language::Js => "JavaScript",
            Language::Jsx => "JSX",
            Language::Python => "Python",
        }
    }

    /// Whether this language emits dotted/relative Python specifiers rather
    /// than TS-style bare/aliased ones. Keeps the two resolver halves
    /// disjoint: a dotted spec never reaches the package heuristic that ate
    /// baseUrl imports in M3, and vice versa.
    pub fn is_python(&self) -> bool {
        matches!(self, Language::Python)
    }
}

/// How a file is treated by the pipeline. Only `Source` and `Test` reach the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum FileClass {
    #[default]
    Source,
    Test,
    Config,
    Generated,
    Vendored,
}

impl FileClass {
    pub fn label(&self) -> &'static str {
        match self {
            FileClass::Source => "source",
            FileClass::Test => "test",
            FileClass::Config => "config",
            FileClass::Generated => "generated",
            FileClass::Vendored => "vendored",
        }
    }

    /// Whether this file participates in parsing, the graph and clustering.
    pub fn is_analyzed(&self) -> bool {
        matches!(self, FileClass::Source | FileClass::Test)
    }
}

#[derive(Debug, Clone)]
pub struct FileRecord {
    pub id: FileId,
    /// Repository-relative path, always forward-slashed.
    pub rel: String,
    pub abs: PathBuf,
    pub language: Language,
    pub bytes: u64,
    pub loc: usize,
    pub class: FileClass,
}

impl FileRecord {
    /// Directory portion of `rel`, empty string at the repository root.
    pub fn dir(&self) -> &str {
        match self.rel.rfind('/') {
            Some(i) => &self.rel[..i],
            None => "",
        }
    }

    pub fn stem(&self) -> &str {
        let base = match self.rel.rfind('/') {
            Some(i) => &self.rel[i + 1..],
            None => &self.rel,
        };
        match base.find('.') {
            Some(i) => &base[..i],
            None => base,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SymbolKind {
    Function,
    Class,
    Interface,
    TypeAlias,
    Enum,
    Const,
    Method,
}

impl SymbolKind {
    /// Ordering weight used when picking which symbols represent a cluster.
    pub fn salience(&self) -> u8 {
        match self {
            SymbolKind::Class => 5,
            SymbolKind::Function => 5,
            SymbolKind::Interface => 4,
            SymbolKind::Enum => 3,
            SymbolKind::TypeAlias => 3,
            SymbolKind::Method => 2,
            SymbolKind::Const => 1,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub exported: bool,
    pub line: usize,
}

impl Symbol {
    /// Case-insensitive stem key used by the semantic-contract join: a TS
    /// `createUser` and a Python `create_user` are the same call shape.
    /// Mirrors `stem` in src/label/validate.rs (plural fold only).
    pub fn stem_key(&self) -> String {
        let lower = self.name.to_ascii_lowercase().replace('_', "");
        let key = if lower.len() > 3 && lower.ends_with('s') && !lower.ends_with("ss") {
            &lower[..lower.len() - 1]
        } else {
            &lower[..]
        };
        key.to_string()
    }
}

/// A module specifier as written in source, before resolution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RawRef {
    pub specifier: String,
    pub line: usize,
}

/// An entry point discovered from framework conventions or manifest fields.
#[derive(Debug, Clone)]
pub struct RouteHint {
    pub file: FileId,
    /// Human-readable form, e.g. "GET /api/users" or "bin: codearch".
    pub label: String,
}

/// Resolution outcome for a single specifier.
#[derive(Debug, Clone)]
pub enum RefTarget {
    Internal(FileId),
    /// Bare specifier that resolved to a declared dependency.
    External(String),
    /// Looked internal (relative or aliased) but no file matched.
    Unresolved(String),
}
