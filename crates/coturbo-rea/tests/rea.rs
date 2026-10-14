use coturbo_rea::{binary, js, site, terms};

#[test]
fn questions_become_search_terms() {
    assert_eq!(terms("Como funciona a busca de notas?"), ["busca", "notas"]);
    assert!(terms("o que é isso").is_empty());
}

#[test]
fn a_native_binary_is_described() {
    let exe = std::env::current_exe().unwrap();
    let data = std::fs::read(&exe).unwrap();
    let r = binary::inspect(&data, "panic").unwrap();
    assert!(r.starts_with("Binary: "), "{r}");
    assert!(r.contains("likely language: Rust"), "{r}");
    assert!(r.contains("Sections: "), "{r}");
    assert!(r.contains("Strings matching"), "{r}");
    assert!(binary::inspect(b"not a binary at all", "").is_err());
    let s = binary::strings(b"\x00\x01hello world\x00ab\x00", 6);
    assert_eq!(s, [(2, "hello world".to_string())]);
}

#[test]
fn javascript_apps_show_ipc_routes_endpoints_and_matches() {
    let main = "const { ipcMain } = require('electron');\nconst express = require('express');\nipcMain.handle('notes:search', async (e, q) => db.search(q));\napp.get('/api/notes', handler);\nfetch('https://api.example.com/v1/sync');\n";
    let renderer = "import { debounce } from 'lodash';\nimport React from 'react';\nexport function search(q) { return ipcRenderer.invoke('notes:search', q) }\n";
    let files = vec![("main.js".to_string(), main.to_string()), ("src/app.jsx".to_string(), renderer.to_string())];
    let pkg = r#"{"name": "notas", "main": "main.js", "dependencies": {"electron": "1", "lodash": "4"}}"#;
    let r = js::inspect_files(&files, Some(pkg), "como funciona a search");
    assert!(r.contains("Package: notas (main: main.js)"), "{r}");
    assert!(r.contains("electron") && r.contains("lodash×1") && r.contains("react×1"), "{r}");
    assert!(r.contains("notes:search (handle)  [main.js:3]") && r.contains("notes:search (invoke)"), "{r}");
    assert!(r.contains("/api/notes  [main.js:4]"), "{r}");
    assert!(r.contains("https://api.example.com/v1/sync"), "{r}");
    assert!(r.contains("src/app.jsx:3:"), "matches carry file:line: {r}");
}

/// A minimal .asar archive with the given files.
fn asar(files: &[(&str, &str)]) -> Vec<u8> {
    let mut entries = serde_json::Map::new();
    let mut body = Vec::new();
    for (name, text) in files {
        entries.insert(name.to_string(), serde_json::json!({"size": text.len(), "offset": body.len().to_string()}));
        body.extend_from_slice(text.as_bytes());
    }
    let json = serde_json::json!({"files": entries}).to_string();
    let json_len = json.len();
    let padded = json_len.div_ceil(4) * 4;
    let header_size = padded + 8;
    let mut out = Vec::new();
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(&(header_size as u32).to_le_bytes());
    out.extend_from_slice(&((header_size - 4) as u32).to_le_bytes());
    out.extend_from_slice(&(json_len as u32).to_le_bytes());
    out.extend_from_slice(json.as_bytes());
    out.resize(8 + header_size, 0);
    out.extend_from_slice(&body);
    out
}

#[test]
fn electron_archives_are_opened() {
    let data = asar(&[("package.json", r#"{"name":"app","main":"index.js"}"#), ("index.js", "ipcMain.on('save-file', f)\n")]);
    let files = js::asar_files(&data).unwrap();
    assert_eq!(files.len(), 2);
    let r = js::inspect_asar(&data, "save").unwrap();
    assert!(r.contains("Electron archive") && r.contains("save-file (on)") && r.contains("Package: app"), "{r}");
    assert!(js::asar_files(b"xx").is_err());
}

#[test]
fn rendered_pages_are_described() {
    let dom = r#"<html><head><title>Notas Rápidas</title><script id="__NEXT_DATA__"></script><script src="/a.js"></script></head>
<body><header><nav></nav></header><main><section id="busca"><h1>Busca <b>instantânea</b></h1><h2>Filtros</h2></section></main><footer></footer><form></form></body></html>"#;
    let r = site::describe(dom);
    assert!(r.contains("Title: Notas Rápidas"), "{r}");
    assert!(r.contains("h1 Busca instantânea") && r.contains("  h2 Filtros"), "{r}");
    assert!(r.contains("header → nav → main → section#busca → footer"), "{r}");
    assert!(r.contains("Next.js"), "{r}");
    assert!(r.contains("Scripts loaded: 1; forms: 1"), "{r}");
}
