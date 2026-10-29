//! Skeletons of JavaScript and TypeScript files: every function body longer
//! than two lines elided to `{ … }`, so a large file costs a fraction of its
//! tokens to read. Signatures, types, imports and exports stay.

use oxc_allocator::Allocator;
use oxc_ast::ast::FunctionBody;
use oxc_ast_visit::Visit;
use oxc_parser::Parser;
use oxc_span::SourceType;

/// Outermost function bodies, as byte ranges.
struct Bodies(Vec<(usize, usize)>);

impl<'a> Visit<'a> for Bodies {
    fn visit_function_body(&mut self, it: &FunctionBody<'a>) {
        // Not walking inside: nested bodies go with their parent.
        self.0.push((it.span.start as usize, it.span.end as usize));
    }
}

pub fn is_js(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    [".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts"]
        .iter()
        .any(|e| p.ends_with(e))
}

/// The skeleton of `src`, or `None` if it does not parse.
pub fn js_skeleton(path: &str, src: &str) -> Option<String> {
    let source_type = SourceType::from_path(path).ok()?;
    let alloc = Allocator::default();
    let parsed = Parser::new(&alloc, src, source_type).parse();
    if parsed.panicked {
        return None;
    }
    let mut bodies = Bodies(Vec::new());
    bodies.visit_program(&parsed.program);
    let mut out = String::with_capacity(src.len() / 3);
    let mut at = 0;
    for (start, end) in bodies.0 {
        let body = &src[start..end];
        if start < at || !body.starts_with('{') || body.lines().count() <= 3 {
            continue;
        }
        out.push_str(&src[at..start]);
        out.push_str("{ … }");
        at = end;
    }
    out.push_str(&src[at..]);
    Some(out)
}
