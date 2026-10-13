use coturbo_ast::{AstError, list_symbols, read_symbol, replace_symbol, insert_symbol, skeleton};

const SRC: &str = r#"use std::fmt;

/// A point.
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub fn new(x: i32, y: i32) -> Self {
        let p = Point { x, y };
        p
    }

    fn norm(&self) -> i32 {
        self.x.abs() + self.y.abs()
    }
}

pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

mod inner {
    pub fn helper() -> u8 {
        42
    }
}
"#;

#[test]
fn skeleton_hides_bodies_keeps_signatures() {
    let s = skeleton(SRC);
    assert!(s.contains("use std::fmt;"));
    assert!(s.contains("pub struct Point {"));
    assert!(s.contains("pub x: i32,"));
    assert!(s.contains("pub fn new(x: i32, y: i32) -> Self { … }"));
    assert!(s.contains("fn norm(&self) -> i32 { … }"));
    assert!(s.contains("pub fn add(a: i32, b: i32) -> i32 { … }"));
    assert!(s.contains("pub fn helper() -> u8 { … }"));
    assert!(!s.contains("a + b"));
    assert!(!s.contains("let p"));
    assert!(s.len() < SRC.len());
}

#[test]
fn skeleton_survives_syntax_errors() {
    let broken = "pub fn ok() -> u8 { 1 }\nfn bad( {\n";
    let s = skeleton(broken);
    assert!(s.contains("pub fn ok() -> u8 { … }"));
}

#[test]
fn lists_symbol_paths() {
    let names: Vec<String> = list_symbols(SRC).into_iter().map(|s| s.path).collect();
    for p in ["Point", "Point::new", "Point::norm", "add", "inner", "inner::helper"] {
        assert!(names.contains(&p.to_string()), "missing {p} in {names:?}");
    }
}

#[test]
fn reads_exact_symbol() {
    let body = read_symbol(SRC, "Point::norm").unwrap();
    assert!(body.starts_with("fn norm(&self) -> i32 {"));
    assert!(body.trim_end().ends_with('}'));
    assert!(!body.contains("pub fn new"));
    assert_eq!(read_symbol(SRC, "nope"), Err(AstError::SymbolNotFound("nope".into())));
}

#[test]
fn read_symbol_includes_doc_comments() {
    let body = read_symbol(SRC, "Point").unwrap();
    assert!(body.starts_with("/// A point."));
}

#[test]
fn replaces_symbol_in_place() {
    let out = replace_symbol(SRC, "add", "pub fn add(a: i32, b: i32) -> i32 {\n    a.wrapping_add(b)\n}").unwrap();
    assert!(out.contains("a.wrapping_add(b)"));
    assert!(!out.contains("    a + b\n"));
    assert!(out.contains("fn norm(&self)"));
    assert!(out.contains("pub fn helper()"));
}

#[test]
fn rejects_replacement_that_breaks_syntax() {
    let err = replace_symbol(SRC, "add", "pub fn add(a: i32 {").unwrap_err();
    assert!(matches!(err, AstError::InvalidEdit(_)));
}

#[test]
fn inserts_after_symbol() {
    let out = insert_symbol(SRC, "Point::norm", "fn dbl(&self) -> i32 {\n    self.x * 2\n}").unwrap();
    let norm = out.find("fn norm").unwrap();
    let dbl = out.find("fn dbl").unwrap();
    assert!(dbl > norm);
    assert!(dbl < out.find("pub fn add").unwrap());
    assert!(read_symbol(&out, "Point::dbl").is_ok());
}
