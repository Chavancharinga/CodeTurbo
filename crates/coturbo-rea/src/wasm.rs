//! WebAssembly modules (`.wasm`): the binary format is a magic header
//! followed by sections. We read imports (what the module calls on its
//! host), exports (what it offers), the counts of functions/memory/globals,
//! the function names from the optional `name` custom section, and the
//! strings in data segments related to the question. Pure Rust, no engine.

/// Unsigned LEB128 at `*p`, advancing it. Stops safely at the end.
fn uleb(d: &[u8], p: &mut usize) -> u64 {
    let mut r = 0u64;
    let mut shift = 0;
    while *p < d.len() && shift < 64 {
        let b = d[*p];
        *p += 1;
        r |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    r
}

/// A length-prefixed UTF-8 name at `*p`.
fn name(d: &[u8], p: &mut usize) -> String {
    let len = uleb(d, p) as usize;
    let end = (*p + len).min(d.len());
    let s = String::from_utf8_lossy(&d[*p..end]).into_owned();
    *p = end;
    s
}

fn kind_label(k: u8) -> &'static str {
    match k {
        0 => "func",
        1 => "table",
        2 => "mem",
        3 => "global",
        _ => "?",
    }
}

/// Whether `data` begins with the WASM magic.
pub fn is_wasm(data: &[u8]) -> bool {
    data.len() >= 8 && &data[..4] == b"\0asm"
}

/// Function names from the `name` custom section (subsection id 1).
fn function_names(content: &[u8]) -> Vec<(u64, String)> {
    let mut p = 0;
    let mut out = Vec::new();
    while p < content.len() {
        let sub_id = content[p];
        p += 1;
        let size = uleb(content, &mut p) as usize;
        let end = (p + size).min(content.len());
        if sub_id == 1 {
            let mut q = p;
            let count = uleb(content, &mut q);
            for _ in 0..count.min(5000) {
                let idx = uleb(content, &mut q);
                let n = name(content, &mut q);
                if !n.is_empty() {
                    out.push((idx, n));
                }
                if q >= end {
                    break;
                }
            }
            return out;
        }
        p = end;
    }
    out
}

/// Report on a WASM module.
pub fn inspect(data: &[u8], question: &str) -> Result<String, String> {
    if !is_wasm(data) {
        return Err("não é um módulo WebAssembly".to_string());
    }
    let version = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let mut r = format!(
        "WebAssembly module: version {version}, {} bytes\n",
        data.len()
    );
    let mut imports: Vec<String> = Vec::new();
    let mut exports: Vec<String> = Vec::new();
    let mut fn_count = 0u64;
    let mut mem = 0u64;
    let mut globals = 0u64;
    let mut tables = 0u64;
    let mut names: Vec<(u64, String)> = Vec::new();
    let mut data_segments = 0u64;

    let mut p = 8;
    while p < data.len() {
        let id = data[p];
        p += 1;
        let size = uleb(data, &mut p) as usize;
        let end = (p + size).min(data.len());
        let content = &data[p..end];
        match id {
            2 => {
                // Imports.
                let mut q = 0;
                let count = uleb(content, &mut q);
                for _ in 0..count.min(5000) {
                    let module = name(content, &mut q);
                    let field = name(content, &mut q);
                    let kind = content.get(q).copied().unwrap_or(0);
                    q += 1;
                    // Skip the descriptor (type index / limits / global type).
                    uleb(content, &mut q);
                    if kind != 0 {
                        uleb(content, &mut q);
                    }
                    imports.push(format!("{module}.{field} ({})", kind_label(kind)));
                    if q >= content.len() {
                        break;
                    }
                }
            }
            3 => {
                let mut q = 0;
                fn_count = uleb(content, &mut q);
            }
            4 => {
                let mut q = 0;
                tables = uleb(content, &mut q);
            }
            5 => {
                let mut q = 0;
                mem = uleb(content, &mut q);
            }
            6 => {
                let mut q = 0;
                globals = uleb(content, &mut q);
            }
            7 => {
                // Exports.
                let mut q = 0;
                let count = uleb(content, &mut q);
                for _ in 0..count.min(5000) {
                    let n = name(content, &mut q);
                    let kind = content.get(q).copied().unwrap_or(0);
                    q += 1;
                    uleb(content, &mut q);
                    exports.push(format!("{n} ({})", kind_label(kind)));
                    if q >= content.len() {
                        break;
                    }
                }
            }
            11 => {
                let mut q = 0;
                data_segments = uleb(content, &mut q);
            }
            0 => {
                // Custom section: name then content.
                let mut q = 0;
                let sec_name = name(content, &mut q);
                if sec_name == "name" {
                    names = function_names(&content[q..]);
                }
            }
            _ => {}
        }
        p = end;
    }

    r.push_str(&format!(
        "Functions: {fn_count} · memory sections: {mem} · globals: {globals} · tables: {tables} · data segments: {data_segments}\n"
    ));
    if !imports.is_empty() {
        // Group imports by host module.
        let mut by_mod: std::collections::BTreeMap<String, usize> = Default::default();
        for i in &imports {
            if let Some(m) = i.split('.').next() {
                *by_mod.entry(m.to_string()).or_insert(0) += 1;
            }
        }
        r.push_str(&format!(
            "Imports ({}, from {}): {}\n",
            imports.len(),
            by_mod.keys().cloned().collect::<Vec<_>>().join(", "),
            imports
                .iter()
                .take(30)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !exports.is_empty() {
        r.push_str(&format!(
            "Exports ({}): {}\n",
            exports.len(),
            exports
                .iter()
                .take(40)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let terms = crate::terms(question);
    if !names.is_empty() {
        let hits: Vec<String> = names
            .iter()
            .filter(|(_, n)| terms.is_empty() || terms.iter().any(|t| n.to_lowercase().contains(t)))
            .take(40)
            .map(|(i, n)| format!("#{i} {n}"))
            .collect();
        r.push_str(&format!(
            "Function names ({} total){}: {}\n",
            names.len(),
            if terms.is_empty() {
                ""
            } else {
                " matching the question"
            },
            hits.join(", ")
        ));
    }
    if !terms.is_empty() {
        let strings = crate::binary::strings(data, 5);
        let hits: Vec<String> = strings
            .iter()
            .filter(|(_, s)| terms.iter().any(|t| s.to_lowercase().contains(t)))
            .take(25)
            .map(|(o, s)| format!("  0x{o:x}: {}", s.chars().take(120).collect::<String>()))
            .collect();
        if !hits.is_empty() {
            r.push_str(&format!(
                "Strings matching {:?}:\n{}\n",
                terms,
                hits.join("\n")
            ));
        }
    }
    r.push_str(
        "Unknowns: the function bodies are stack-machine bytecode; for readable pseudocode use a WASM decompiler (e.g. the external REA, or wasm2c/wasm-decompile).",
    );
    Ok(r)
}
