//! Embeds the skills in `builtin/` (Markdown and licences) into the binary.

use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("builtin");
    println!("cargo:rerun-if-changed=builtin");
    let mut files = Vec::new();
    walk(&root, &mut files);
    let mut code = String::from("&[\n");
    for f in files {
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        code.push_str(&format!("    ({rel:?}, include_str!({:?})),\n", f.to_string_lossy()));
    }
    code.push(']');
    std::fs::write(PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("builtin.rs"), code).unwrap();
}
