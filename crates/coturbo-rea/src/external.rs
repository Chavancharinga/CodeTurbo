//! The external REA (`morluto/rea`), used to deepen analysis when it is
//! installed: it reaches engines our native code does not (Ghidra/IDA/Hopper
//! decompilation, Playwright/Electron runtime capture, APK via JADX). We
//! detect it and point the agent to the exact commands for the target; the
//! agent runs them with bash. Absent, our native analysis stands alone.

use std::path::Path;

/// Whether the external `rea` CLI is on PATH.
pub fn installed() -> bool {
    let exts: &[&str] = if cfg!(windows) { &["cmd", "bat", "exe", ""] } else { &[""] };
    std::env::var_os("PATH").is_some_and(|p| {
        std::env::split_paths(&p).any(|d| {
            exts.iter().any(|e| {
                let f = if e.is_empty() { d.join("rea") } else { d.join(format!("rea.{e}")) };
                f.is_file()
            })
        })
    })
}

/// What the external REA can add for a target, as ready-to-run commands.
/// `kind` is "site", "js", "binary" or "other"; `target` is the URL or path.
pub fn suggestions(kind: &str, target: &str) -> String {
    if !installed() {
        return String::new();
    }
    let cmds: &[(&str, &str)] = match kind {
        "site" => &[
            ("rea analyze-web-bundle {t}", "captura e reconstrói o bundle da página"),
            ("rea capture-browser-scenario {t}", "roda um cenário no navegador e devolve evidências"),
        ],
        "js" => &[
            ("rea analyze-javascript-application {t} --json", "reconstrói o app JavaScript/Electron estaticamente"),
            ("rea function {t} <nome>", "analisa uma função específica com evidências"),
            ("rea decompile {t}", "lê uma parte do app como código"),
        ],
        "binary" => &[
            ("rea function {t} <nome>", "decompila uma função (precisa de Ghidra/Hopper/IDA)"),
            ("rea decompile {t}", "pseudocódigo C das funções (precisa de Ghidra/Hopper/IDA)"),
        ],
        _ => &[("rea analyze {t}", "visão geral do alvo com evidências")],
    };
    let list = cmds.iter().map(|(c, d)| format!("  {}  — {d}", c.replace("{t}", target))).collect::<Vec<_>>().join("\n");
    format!(
        "\n\nAnálise mais profunda disponível pelo REA externo (instalado). Rode pelo bash quando precisar de mais do que a análise estática acima:\n{list}\n\
         (Para pseudocódigo C de binário nativo, instale o Ghidra e defina GHIDRA_INSTALL_DIR; o `rea doctor` confirma.)"
    )
}

/// Runs `rea <args>` in `cwd`, returning stdout, with a timeout. Used when a
/// tool wants to delegate directly instead of suggesting a command.
pub async fn run(cwd: &Path, args: &[&str], timeout_secs: u64) -> Result<String, String> {
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        tokio::process::Command::new(if cfg!(windows) { "rea.cmd" } else { "rea" })
            .args(args)
            .current_dir(cwd)
            .output(),
    )
    .await
    .map_err(|_| format!("o rea externo não respondeu em {timeout_secs}s"))?
    .map_err(|e| format!("não foi possível rodar o rea: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("rea falhou: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}
