//! Shared types across pipeline stages.

use std::path::{Path, PathBuf};

pub type FileId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Ts,
    Tsx,
    Js,
    Jsx,
}

impl Language {
    pub fn from_path(p: &Path) -> Option<Language> {
        match p.extension().and_then(|e| e.to_str())? {
            "ts" | "mts" | "cts" => Some(Language::Ts),
            "tsx" => Some(Language::Tsx),
            "js" | "mjs" | "cjs" => Some(Language::Js),
            "jsx" => Some(Language::Jsx),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Language::Ts => "TypeScript",
            Language::Tsx => "TSX",
            Language::Js => "JavaScript",
            Language::Jsx => "JSX",
        }
    }
}

/// How a file is treated by the pipeline. Only `Source` and `Test` reach the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileClass {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub exported: bool,
    pub line: usize,
}

/// A module specifier as written in source, before resolution.
#[derive(Debug, Clone)]
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
