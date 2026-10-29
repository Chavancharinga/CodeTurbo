//! Detects edits that make checks pass without fixing anything: fewer tests,
//! new `#[ignore]`/`.skip`, new lint suppressions, new `todo!()`, and tests
//! whose body was emptied. Compares each edited file before and after the task.
//! Rust is analysed on the syntax tree, so strings and comments never count.

use ra_ap_syntax::{
    AstNode, Edition, SourceFile,
    ast::{self, HasAttrs, HasName},
};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheatKind {
    FewerTests,
    SkippedTest,
    LintSuppressed,
    Placeholder,
    EmptiedTest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub kind: CheatKind,
    pub detail: String,
}

impl Finding {
    pub fn render(&self) -> String {
        format!("{}: {}", self.path, self.detail)
    }
}

/// Content of each edited file before its first edit in the task (`None`:
/// the file did not exist).
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    before: BTreeMap<String, Option<String>>,
}

impl Snapshot {
    /// Records `rel` the first time it is about to be edited.
    pub fn record(&mut self, root: &Path, rel: &str) {
        let rel = rel.replace('\\', "/");
        if let std::collections::btree_map::Entry::Vacant(e) = self.before.entry(rel) {
            let content = std::fs::read_to_string(root.join(e.key())).ok();
            e.insert(content);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.before.is_empty()
    }

    pub fn clear(&mut self) {
        self.before.clear();
    }

    pub fn findings(&self, root: &Path) -> Vec<Finding> {
        let mut out = Vec::new();
        for (rel, before) in &self.before {
            let after = std::fs::read_to_string(root.join(rel)).unwrap_or_default();
            out.extend(compare(rel, before.as_deref().unwrap_or(""), &after));
        }
        out
    }
}

/// Counts that should never grow (or, for tests, shrink) unnoticed.
#[derive(Debug, Default, PartialEq, Eq)]
struct Profile {
    tests: usize,
    skipped: usize,
    suppressed: usize,
    placeholders: usize,
    /// Test name → body is trivially empty.
    test_bodies: HashMap<String, bool>,
}

fn lang(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?;
    Some(match ext {
        "rs" => "rust",
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts" => "js",
        "py" => "python",
        _ => return None,
    })
}

/// Attribute path, e.g. `tokio::test` for `#[tokio::test(flavor = "x")]`.
fn attr_path(attr: &ast::Attr) -> String {
    let t = attr.syntax().text().to_string();
    let t = t
        .trim_start_matches("#!")
        .trim_start_matches('#')
        .trim_start_matches('[')
        .trim();
    t.split(|c: char| c == '(' || c == ']' || c == '=' || c.is_whitespace())
        .next()
        .unwrap_or("")
        .to_string()
}

fn trivial_body(body: &str) -> bool {
    let inner: String = body
        .trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .split_whitespace()
        .collect();
    inner.is_empty() || inner == "assert!(true)" || inner == "assert!(true);"
}

fn rust_profile(src: &str) -> Profile {
    let file = SourceFile::parse(src, Edition::Edition2024).tree();
    let mut p = Profile::default();
    for node in file.syntax().descendants() {
        if let Some(attr) = ast::Attr::cast(node.clone()) {
            match attr_path(&attr).as_str() {
                "ignore" => p.skipped += 1,
                "allow" | "expect" => p.suppressed += 1,
                _ => {}
            }
        } else if let Some(mc) = ast::MacroCall::cast(node.clone()) {
            let path = mc
                .path()
                .map(|p| p.syntax().text().to_string())
                .unwrap_or_default();
            if matches!(path.as_str(), "todo" | "unimplemented") {
                p.placeholders += 1;
            }
        } else if let Some(f) = ast::Fn::cast(node) {
            let is_test = f.attrs().any(|a| {
                let path = attr_path(&a);
                path == "test" || path.ends_with("::test")
            });
            if is_test {
                p.tests += 1;
                let name = f.name().map(|n| n.text().to_string()).unwrap_or_default();
                let body = f
                    .body()
                    .map(|b| b.syntax().text().to_string())
                    .unwrap_or_default();
                p.test_bodies.insert(name, trivial_body(&body));
            }
        }
    }
    p
}

fn count(re: &str, src: &str) -> usize {
    regex::Regex::new(re)
        .expect("valid regex")
        .find_iter(src)
        .count()
}

fn text_profile(lang: &str, src: &str) -> Profile {
    match lang {
        "js" => Profile {
            // Skipped tests still count as tests: skipping is reported on its own.
            tests: count(r"\b(?:x?it|x?test)(?:\.only|\.skip)?\s*\(", src),
            skipped: count(
                r"\b(?:it|test|describe)\.skip\s*\(|\bx(?:it|describe|test)\s*\(",
                src,
            ),
            suppressed: count(
                r"eslint-disable|@ts-ignore|@ts-expect-error|@ts-nocheck",
                src,
            ),
            placeholders: count(r"expect\(\s*true\s*\)\.toBe\(\s*true\s*\)", src),
            test_bodies: HashMap::new(),
        },
        _ => Profile {
            tests: count(r"(?m)^\s*(?:async\s+)?def\s+test_\w*", src),
            skipped: count(r"@pytest\.mark\.skip|@unittest\.skip", src),
            suppressed: count(r"#\s*type:\s*ignore|#\s*noqa|#\s*pyright:\s*ignore", src),
            placeholders: count(r"(?m)^\s*assert\s+True\s*$", src),
            test_bodies: HashMap::new(),
        },
    }
}

fn profile(lang: &str, src: &str) -> Profile {
    if lang == "rust" {
        rust_profile(src)
    } else {
        text_profile(lang, src)
    }
}

/// Findings for one file edited from `before` to `after`.
pub fn compare(path: &str, before: &str, after: &str) -> Vec<Finding> {
    let Some(lang) = lang(path) else {
        return Vec::new();
    };
    let (b, a) = (profile(lang, before), profile(lang, after));
    let mut out = Vec::new();
    let mut push = |kind, detail: String| {
        out.push(Finding {
            path: path.to_string(),
            kind,
            detail,
        })
    };
    if a.tests < b.tests {
        push(
            CheatKind::FewerTests,
            format!("tests dropped from {} to {}", b.tests, a.tests),
        );
    }
    if a.skipped > b.skipped {
        push(
            CheatKind::SkippedTest,
            format!("{} new skipped/ignored test(s)", a.skipped - b.skipped),
        );
    }
    if a.suppressed > b.suppressed {
        push(
            CheatKind::LintSuppressed,
            format!(
                "{} new lint/type-check suppression(s)",
                a.suppressed - b.suppressed
            ),
        );
    }
    if a.placeholders > b.placeholders {
        let what = if lang == "rust" {
            "todo!/unimplemented!"
        } else {
            "always-true assertion"
        };
        push(
            CheatKind::Placeholder,
            format!("{} new {what}", a.placeholders - b.placeholders),
        );
    }
    let mut emptied: Vec<&String> = a
        .test_bodies
        .iter()
        .filter(|(name, empty)| **empty && b.test_bodies.get(*name) == Some(&false))
        .map(|(name, _)| name)
        .collect();
    emptied.sort();
    for name in emptied {
        push(CheatKind::EmptiedTest, format!("test `{name}` was emptied"));
    }
    out
}
