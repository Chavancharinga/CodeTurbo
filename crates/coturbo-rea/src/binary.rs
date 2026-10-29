//! Native binaries: format, architecture, likely language, imports,
//! exports, sections, and the strings inside that relate to the question.

use object::{Object, ObjectSection, ObjectSymbol};

/// Printable ASCII runs of at least `min` bytes, with their offsets.
pub fn strings(data: &[u8], min: usize) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, &b) in data.iter().enumerate() {
        let printable = (0x20..0x7f).contains(&b) || b == b'\t';
        match (printable, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                if i - s >= min {
                    out.push((s, String::from_utf8_lossy(&data[s..i]).into_owned()));
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start
        && data.len() - s >= min
    {
        out.push((s, String::from_utf8_lossy(&data[s..]).into_owned()));
    }
    out
}

fn language(data: &[u8], strings: &[(usize, String)]) -> &'static str {
    let has = |needle: &[u8]| data.windows(needle.len()).any(|w| w == needle);
    // Compiler-embedded paths and markers, not words that could be anywhere.
    let rust_path = regex::Regex::new(r"/rustc/[0-9a-f]{40}/").expect("valid regex");
    if strings.iter().any(|(_, s)| rust_path.is_match(s)) {
        "Rust"
    } else if has(b"\xff Go buildinf:") {
        "Go"
    } else if strings
        .iter()
        .any(|(_, s)| s == "mscorlib" || s.starts_with("System.Private.CoreLib"))
    {
        ".NET"
    } else if strings
        .iter()
        .any(|(_, s)| s.starts_with("Electron/") || s == "node.dll")
    {
        "Electron/Node"
    } else if strings
        .iter()
        .any(|(_, s)| s.starts_with("MSVCP") || s.starts_with("libstdc++"))
    {
        "C++"
    } else {
        "desconhecida (C ou outra)"
    }
}

/// Report on a binary file.
pub fn inspect(data: &[u8], question: &str) -> Result<String, String> {
    let file = object::File::parse(data)
        .map_err(|e| format!("formato não reconhecido como binário: {e}"))?;
    let all = strings(data, 6);
    let mut r = format!(
        "Binary: {:?}, {:?}, {} bytes, likely language: {}, entry 0x{:x}\n",
        file.format(),
        file.architecture(),
        data.len(),
        language(data, &all),
        file.entry()
    );
    let sections: Vec<String> = file
        .sections()
        .filter_map(|s| Some(format!("{} ({}b)", s.name().ok()?, s.size())))
        .take(16)
        .collect();
    r.push_str(&format!("Sections: {}\n", sections.join(", ")));
    if let Ok(imports) = file.imports() {
        let mut by_lib: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for i in &imports {
            let lib = String::from_utf8_lossy(i.library()).into_owned();
            by_lib
                .entry(lib)
                .or_default()
                .push(crate::demangle::demangle_short(&String::from_utf8_lossy(
                    i.name(),
                )));
        }
        if !by_lib.is_empty() {
            r.push_str("Imports:\n");
            for (lib, names) in by_lib.iter().take(15) {
                let shown: Vec<&String> = names.iter().take(12).collect();
                r.push_str(&format!(
                    "  {lib}: {}{}\n",
                    shown
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    if names.len() > 12 { ", …" } else { "" }
                ));
            }
        }
    }
    if let Ok(exports) = file.exports() {
        let names: Vec<String> = exports
            .iter()
            .take(30)
            .map(|e| crate::demangle::demangle_short(&String::from_utf8_lossy(e.name())))
            .collect();
        if !names.is_empty() {
            r.push_str(&format!(
                "Exports ({}): {}\n",
                exports.len(),
                names.join(", ")
            ));
        }
    }
    let terms = crate::terms(question);
    let symbols: Vec<String> = file
        .symbols()
        .filter_map(|s| s.name().ok().map(str::to_string))
        .filter(|n| !terms.is_empty() && terms.iter().any(|t| n.to_lowercase().contains(t)))
        .take(25)
        .collect();
    if !symbols.is_empty() {
        r.push_str(&format!(
            "Symbols matching the question: {}\n",
            symbols.join(", ")
        ));
    }
    let urls: Vec<&String> = all
        .iter()
        .map(|(_, s)| s)
        .filter(|s| s.contains("http://") || s.contains("https://"))
        .take(12)
        .collect();
    if !urls.is_empty() {
        r.push_str(&format!(
            "URLs inside: {}\n",
            urls.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    if !terms.is_empty() {
        let hits: Vec<String> = all
            .iter()
            .filter(|(_, s)| terms.iter().any(|t| s.to_lowercase().contains(t)))
            .take(30)
            .map(|(o, s)| format!("  0x{o:x}: {}", s.chars().take(140).collect::<String>()))
            .collect();
        r.push_str(&format!(
            "Strings matching {:?} ({} shown):\n{}\n",
            terms,
            hits.len(),
            hits.join("\n")
        ));
    }
    r.push_str("Unknowns: control flow and full C pseudocode need a decompiler.");
    Ok(r)
}
