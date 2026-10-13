//! Rust source navigation and symbol-level editing, built on rust-analyzer's
//! lossless, error-tolerant parser.

use ra_ap_syntax::{
    AstNode, Edition, SourceFile,
    ast::{self, HasModuleItem, HasName},
};
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AstError {
    #[error("symbol not found: {0}")]
    SymbolNotFound(String),
    #[error("edit would introduce syntax errors: {0}")]
    InvalidEdit(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// Path such as `Point::new` or `inner::helper`.
    pub path: String,
    pub kind: &'static str,
    /// Byte range in the source, including leading doc comments and attributes.
    pub range: Range<usize>,
}

fn parse(src: &str) -> ra_ap_syntax::Parse<SourceFile> {
    SourceFile::parse(src, Edition::Edition2024)
}

fn range_of(node: &impl AstNode) -> Range<usize> {
    let r = node.syntax().text_range();
    usize::from(r.start())..usize::from(r.end())
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() { name.to_string() } else { format!("{prefix}::{name}") }
}

/// Name of the type an `impl` block targets, without generics (`Foo<T>` -> `Foo`).
fn impl_target(imp: &ast::Impl) -> Option<String> {
    let ty = imp.self_ty()?.syntax().text().to_string();
    let base = ty.split('<').next().unwrap_or(&ty).trim();
    Some(base.rsplit("::").next().unwrap_or(base).to_string())
}

fn collect_assoc(list: Option<ast::AssocItemList>, prefix: &str, out: &mut Vec<Symbol>) {
    let Some(list) = list else { return };
    for item in list.assoc_items() {
        let (name, kind) = match &item {
            ast::AssocItem::Fn(f) => (f.name(), "fn"),
            ast::AssocItem::Const(c) => (c.name(), "const"),
            ast::AssocItem::TypeAlias(t) => (t.name(), "type"),
            ast::AssocItem::MacroCall(_) => continue,
        };
        if let Some(name) = name {
            out.push(Symbol { path: join(prefix, &name.text()), kind, range: range_of(&item) });
        }
    }
}

fn collect_items(items: impl Iterator<Item = ast::Item>, prefix: &str, out: &mut Vec<Symbol>) {
    for item in items {
        let named = |n: Option<ast::Name>, kind: &'static str, out: &mut Vec<Symbol>| {
            if let Some(n) = n {
                out.push(Symbol { path: join(prefix, &n.text()), kind, range: range_of(&item) });
            }
        };
        match &item {
            ast::Item::Fn(f) => named(f.name(), "fn", out),
            ast::Item::Struct(s) => named(s.name(), "struct", out),
            ast::Item::Enum(e) => named(e.name(), "enum", out),
            ast::Item::Union(u) => named(u.name(), "union", out),
            ast::Item::Const(c) => named(c.name(), "const", out),
            ast::Item::Static(s) => named(s.name(), "static", out),
            ast::Item::TypeAlias(t) => named(t.name(), "type", out),
            ast::Item::MacroRules(m) => named(m.name(), "macro", out),
            ast::Item::Trait(t) => {
                named(t.name(), "trait", out);
                if let Some(n) = t.name() {
                    collect_assoc(t.assoc_item_list(), &join(prefix, &n.text()), out);
                }
            }
            ast::Item::Impl(imp) => {
                if let Some(target) = impl_target(imp) {
                    collect_assoc(imp.assoc_item_list(), &join(prefix, &target), out);
                }
            }
            ast::Item::Module(m) => {
                named(m.name(), "mod", out);
                if let (Some(n), Some(list)) = (m.name(), m.item_list()) {
                    collect_items(list.items(), &join(prefix, &n.text()), out);
                }
            }
            _ => {}
        }
    }
}

/// All addressable symbols in the file, in source order.
pub fn list_symbols(src: &str) -> Vec<Symbol> {
    let mut out = Vec::new();
    collect_items(parse(src).tree().items(), "", &mut out);
    out
}

fn find(src: &str, path: &str) -> Result<Symbol, AstError> {
    list_symbols(src)
        .into_iter()
        .find(|s| s.path == path)
        .ok_or_else(|| AstError::SymbolNotFound(path.to_string()))
}

/// The file with every function body replaced by `{ … }`.
/// Works on files with syntax errors; unparseable regions are kept verbatim.
pub fn skeleton(src: &str) -> String {
    let file = parse(src).tree();
    let mut bodies: Vec<Range<usize>> = file
        .syntax()
        .descendants()
        .filter_map(ast::Fn::cast)
        .filter_map(|f| f.body())
        .map(|b| range_of(&b))
        .collect();
    bodies.sort_by_key(|r| r.start);

    let mut out = String::with_capacity(src.len() / 2);
    let mut pos = 0;
    for r in bodies {
        if r.start < pos {
            continue; // nested inside a body already elided
        }
        out.push_str(&src[pos..r.start]);
        out.push_str("{ … }");
        pos = r.end;
    }
    out.push_str(&src[pos..]);
    out
}

/// Exact source text of one symbol, including its doc comments and attributes.
pub fn read_symbol(src: &str, path: &str) -> Result<String, AstError> {
    Ok(src[find(src, path)?.range].to_string())
}

/// Leading whitespace of the line containing byte `at`.
fn indent_at(src: &str, at: usize) -> &str {
    let line_start = src[..at].rfind('\n').map_or(0, |i| i + 1);
    let line = &src[line_start..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// Indents every line after the first, assuming `code` is written at column 0.
fn reindent(code: &str, indent: &str) -> String {
    let code = code.trim_matches('\n');
    code.lines()
        .enumerate()
        .map(|(i, l)| if i == 0 || l.is_empty() { l.to_string() } else { format!("{indent}{l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

fn checked(original: &str, edited: String) -> Result<String, AstError> {
    let before = parse(original).errors().len();
    let errors = parse(&edited).errors();
    if errors.len() > before {
        let msg = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
        return Err(AstError::InvalidEdit(msg));
    }
    Ok(edited)
}

/// Replaces a symbol (doc comments and attributes included) with `new_code`.
pub fn replace_symbol(src: &str, path: &str, new_code: &str) -> Result<String, AstError> {
    let sym = find(src, path)?;
    let code = reindent(new_code, indent_at(src, sym.range.start));
    let edited = format!("{}{}{}", &src[..sym.range.start], code, &src[sym.range.end..]);
    checked(src, edited)
}

/// Inserts `code` as a sibling right after the symbol at `after_path`.
pub fn insert_symbol(src: &str, after_path: &str, code: &str) -> Result<String, AstError> {
    let sym = find(src, after_path)?;
    let indent = indent_at(src, sym.range.start);
    let code = reindent(code, indent);
    let edited = format!("{}\n\n{indent}{code}{}", &src[..sym.range.end], &src[sym.range.end..]);
    checked(src, edited)
}
