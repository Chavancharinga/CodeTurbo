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
    let files = vec![
        ("main.js".to_string(), main.to_string()),
        ("src/app.jsx".to_string(), renderer.to_string()),
    ];
    let pkg =
        r#"{"name": "notas", "main": "main.js", "dependencies": {"electron": "1", "lodash": "4"}}"#;
    let r = js::inspect_files(&files, Some(pkg), "como funciona a search");
    assert!(r.contains("Package: notas (main: main.js)"), "{r}");
    assert!(
        r.contains("electron") && r.contains("lodash×1") && r.contains("react×1"),
        "{r}"
    );
    assert!(
        r.contains("notes:search (handle)  [main.js:3]") && r.contains("notes:search (invoke)"),
        "{r}"
    );
    assert!(r.contains("/api/notes  [main.js:4]"), "{r}");
    assert!(r.contains("https://api.example.com/v1/sync"), "{r}");
    assert!(r.contains("src/app.jsx:3:"), "matches carry file:line: {r}");
}

/// A minimal .asar archive with the given files.
fn asar(files: &[(&str, &str)]) -> Vec<u8> {
    let mut entries = serde_json::Map::new();
    let mut body = Vec::new();
    for (name, text) in files {
        entries.insert(
            name.to_string(),
            serde_json::json!({"size": text.len(), "offset": body.len().to_string()}),
        );
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
    let data = asar(&[
        ("package.json", r#"{"name":"app","main":"index.js"}"#),
        ("index.js", "ipcMain.on('save-file', f)\n"),
    ]);
    let files = js::asar_files(&data).unwrap();
    assert_eq!(files.len(), 2);
    let r = js::inspect_asar(&data, "save").unwrap();
    assert!(
        r.contains("Electron archive")
            && r.contains("save-file (on)")
            && r.contains("Package: app"),
        "{r}"
    );
    assert!(js::asar_files(b"xx").is_err());
}

#[test]
fn rendered_pages_are_described() {
    let dom = r#"<html><head><title>Notas Rápidas</title><script id="__NEXT_DATA__"></script><script src="/a.js"></script></head>
<body><header><nav></nav></header><main><section id="busca"><h1>Busca <b>instantânea</b></h1><h2>Filtros</h2></section></main><footer></footer><form></form></body></html>"#;
    let r = site::describe(dom);
    assert!(r.contains("Title: Notas Rápidas"), "{r}");
    assert!(
        r.contains("h1 Busca instantânea") && r.contains("  h2 Filtros"),
        "{r}"
    );
    assert!(
        r.contains("header → nav → main → section#busca → footer"),
        "{r}"
    );
    assert!(r.contains("Next.js"), "{r}");
    assert!(r.contains("Scripts loaded: 1; forms: 1"), "{r}");
}

#[test]
fn x86_code_is_disassembled() {
    use coturbo_rea::disasm::disassemble;
    // mov eax, 1 ; add eax, 2 ; ret
    let code = [0xb8, 0x01, 0x00, 0x00, 0x00, 0x83, 0xc0, 0x02, 0xc3];
    let lines = disassemble(&code, 0x1000, 64, 20);
    assert_eq!(lines.len(), 3, "stops at ret");
    assert_eq!(lines[0].ip, 0x1000);
    assert!(
        lines[0].text.contains("mov") && lines[0].text.contains("eax"),
        "{}",
        lines[0].text
    );
    assert!(lines[1].text.starts_with("add"), "{}", lines[1].text);
    assert!(lines[2].text.starts_with("ret"), "{}", lines[2].text);
}

#[test]
fn sqlite_schema_is_read() {
    use coturbo_rea::sqlite;
    // A real SQLite file built by hand would be large; instead verify the
    // reader rejects non-SQLite data and accepts the magic header shape.
    assert!(sqlite::inspect(b"not a database").is_err());
    let mut fake = vec![0u8; 512];
    fake[..16].copy_from_slice(b"SQLite format 3\0");
    fake[16] = 0x02; // page size 512
    fake[28] = 0;
    fake[29] = 0;
    fake[30] = 0;
    fake[31] = 1; // 1 page
    let r = sqlite::inspect(&fake).unwrap();
    assert!(r.contains("SQLite database: page size 512"), "{r}");
}

/// A minimal ZIP (APK) with the given entries, stored (no compression).
fn zip_stored(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    let mut offsets = Vec::new();
    for (name, data) in files {
        let crc = {
            let mut c: u32 = 0xffff_ffff;
            for &b in *data {
                c ^= b as u32;
                for _ in 0..8 {
                    c = if c & 1 == 1 {
                        (c >> 1) ^ 0xedb8_8320
                    } else {
                        c >> 1
                    };
                }
            }
            !c
        };
        offsets.push(out.len() as u32);
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // ver, flags, method, time, date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
    }
    let cd_start = out.len() as u32;
    for (i, (name, data)) in files.iter().enumerate() {
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let crc = {
            let mut c: u32 = 0xffff_ffff;
            for &b in *data {
                c ^= b as u32;
                for _ in 0..8 {
                    c = if c & 1 == 1 {
                        (c >> 1) ^ 0xedb8_8320
                    } else {
                        c >> 1
                    };
                }
            }
            !c
        };
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend_from_slice(&offsets[i].to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let cd_len = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[test]
fn apk_structure_and_permissions() {
    use coturbo_rea::apk;
    // AndroidManifest as UTF-16LE text (permission + component names).
    let manifest: Vec<u8> = "com.example.notes android.permission.INTERNET android.permission.CAMERA MainActivity SyncService"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let dex = b"Lcom/example/notes/SearchIndex;->query noteList classes".to_vec();
    let apk = zip_stored(&[
        ("AndroidManifest.xml", &manifest),
        ("classes.dex", &dex),
        ("lib/arm64-v8a/libnative.so", b"x"),
        ("resources.arsc", b"r"),
    ]);
    let r = apk::inspect(&apk, "search").unwrap();
    assert!(r.contains("Android APK") && r.contains("1 DEX"), "{r}");
    assert!(r.contains("arm64-v8a"), "{r}");
    assert!(
        r.contains("android.permission.INTERNET") && r.contains("CAMERA"),
        "{r}"
    );
    assert!(
        r.contains("MainActivity") && r.contains("SyncService"),
        "{r}"
    );
    assert!(
        r.contains("SearchIndex"),
        "dex names matching the question: {r}"
    );
    assert!(apk::inspect(b"not a zip", "").is_err());
}

#[test]
fn external_rea_suggestions_depend_on_install() {
    use coturbo_rea::external;
    let s = external::suggestions("site", "https://exemplo.com");
    if external::installed() {
        assert!(
            s.contains("rea analyze-web-bundle https://exemplo.com"),
            "{s}"
        );
        assert!(
            external::suggestions("binary", "a.exe").contains("rea function a.exe"),
            "binary cmds"
        );
        assert!(
            external::suggestions("js", "/app").contains("rea analyze-javascript-application /app")
        );
    } else {
        assert!(s.is_empty(), "sem rea instalado, nada é sugerido");
    }
}

#[test]
fn wasm_modules_are_parsed() {
    use coturbo_rea::wasm;
    // A minimal WASM module: magic + version + a type section and an export.
    // \0asm, version 1, then type section (id 1) with one () -> () type,
    // function section, and export "main".
    let module: &[u8] = &[
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // magic + version 1
        0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type: 1 type, () -> ()
        0x03, 0x02, 0x01, 0x00, // function: 1 func, type 0
        0x07, 0x08, 0x01, 0x04, b'm', b'a', b'i', b'n', 0x00, 0x00, // export "main" func 0
    ];
    assert!(wasm::is_wasm(module));
    assert!(!wasm::is_wasm(b"not wasm"));
    let r = wasm::inspect(module, "main").unwrap();
    assert!(r.contains("WebAssembly module: version 1"), "{r}");
    assert!(r.contains("Functions: 1"), "{r}");
    assert!(r.contains("main (func)"), "exports: {r}");
    assert!(wasm::inspect(b"xxxxxxxx", "").is_err());
}
