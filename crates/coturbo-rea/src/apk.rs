//! Android APKs (ZIP archives): the package, the permissions and components
//! from the binary `AndroidManifest.xml`, the DEX classes, and resources.
//! Enough to see what an app can do and how it is put together, without the
//! Android toolchain.

use std::io::Read;

/// Printable strings of at least `min` bytes from a binary blob.
fn strings(data: &[u8], min: usize) -> Vec<String> {
    crate::binary::strings(data, min).into_iter().map(|(_, s)| s).collect()
}

/// The readable strings in the binary `AndroidManifest.xml`: Android encodes
/// it as a string pool followed by tags, so the names come through as UTF-16.
fn manifest_strings(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    // UTF-16LE runs: the manifest's string pool is UTF-16.
    let mut buf = String::new();
    let mut i = 0;
    while i + 1 < data.len() {
        let c = u16::from_le_bytes([data[i], data[i + 1]]);
        if (0x20..0x7f).contains(&c) {
            buf.push(c as u8 as char);
        } else {
            if buf.len() >= 3 {
                out.push(std::mem::take(&mut buf));
            } else {
                buf.clear();
            }
        }
        i += 2;
    }
    if buf.len() >= 3 {
        out.push(buf);
    }
    out
}

/// Report on an APK.
pub fn inspect(data: &[u8], question: &str) -> Result<String, String> {
    let cursor = std::io::Cursor::new(data);
    let mut zip = zip::ZipArchive::new(cursor).map_err(|e| format!("não é um APK/ZIP válido: {e}"))?;
    let mut names = Vec::new();
    let mut dex = 0;
    let mut manifest = Vec::new();
    let mut libs = std::collections::BTreeSet::new();
    for i in 0..zip.len() {
        let mut f = match zip.by_index(i) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let name = match f.name() {
            Ok(n) => n.to_string(),
            Err(_) => continue,
        };
        if name == "AndroidManifest.xml" {
            let _ = f.read_to_end(&mut manifest);
        } else if name.ends_with(".dex") {
            dex += 1;
        } else if let Some(lib) = name.strip_prefix("lib/")
            && let Some(arch) = lib.split('/').next()
        {
            libs.insert(arch.to_string());
        }
        names.push(name);
    }
    let mut r = format!("Android APK: {} entries, {dex} DEX file(s)\n", names.len());
    if !libs.is_empty() {
        r.push_str(&format!("Native ABIs: {}\n", libs.into_iter().collect::<Vec<_>>().join(", ")));
    }
    if !manifest.is_empty() {
        let m = manifest_strings(&manifest);
        let pick = |pred: &dyn Fn(&&String) -> bool| m.iter().filter(|s| pred(s)).cloned().collect::<Vec<_>>();
        let perms = pick(&|s| s.starts_with("android.permission.") || s.contains(".permission."));
        if let Some(pkg) = m.iter().find(|s| s.contains('.') && !s.contains('/') && !s.contains(' ') && s.chars().next().is_some_and(char::is_alphabetic) && !s.starts_with("android")) {
            r.push_str(&format!("Likely package: {pkg}\n"));
        }
        if !perms.is_empty() {
            r.push_str(&format!("Permissions ({}): {}\n", perms.len(), perms.iter().take(30).cloned().collect::<Vec<_>>().join(", ")));
        }
        let comps = pick(&|s| s.ends_with("Activity") || s.ends_with("Service") || s.ends_with("Receiver") || s.ends_with("Provider"));
        if !comps.is_empty() {
            r.push_str(&format!("Components: {}\n", comps.iter().take(25).cloned().collect::<Vec<_>>().join(", ")));
        }
    }
    let terms = crate::terms(question);
    if !terms.is_empty() {
        // Class and method names inside the DEX files relate to the question.
        let mut hits = std::collections::BTreeSet::new();
        for i in 0..zip.len() {
            if hits.len() >= 40 {
                break;
            }
            let Ok(mut f) = zip.by_index(i) else { continue };
            if !f.name().is_ok_and(|n| n.ends_with(".dex")) {
                continue;
            }
            let mut bytes = Vec::new();
            if f.read_to_end(&mut bytes).is_err() {
                continue;
            }
            for s in strings(&bytes, 5) {
                let lower = s.to_lowercase();
                if (s.contains('/') || s.contains('.')) && terms.iter().any(|t| lower.contains(t)) {
                    hits.insert(s);
                    if hits.len() >= 40 {
                        break;
                    }
                }
            }
        }
        if !hits.is_empty() {
            r.push_str(&format!("Classes/methods matching {:?}:\n{}\n", terms, hits.iter().take(40).map(|s| format!("  {s}")).collect::<Vec<_>>().join("\n")));
        }
    }
    r.push_str("Unknowns: decompiled Java/Kotlin needs JADX (external REA); this lists structure and names only.");
    Ok(r)
}
