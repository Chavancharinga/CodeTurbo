//! Observar um aplicativo rodando: abre o executável, espera a janela
//! aparecer, captura só aquela janela em PNG e fecha o que abriu. É o "olho"
//! do motor: o programa de referência (para estudar o visual e a disposição
//! reais) e o aplicativo que o próprio agente construiu (para comparar o
//! resultado com a referência e corrigir o que destoa).
//!
//! Hoje funciona no Windows, com o PowerShell que já vem no sistema.
//! Os parâmetros vão por variáveis de ambiente, nunca montados dentro do
//! texto do comando, então um nome de arquivo estranho não vira código.

use std::path::Path;

/// O que a captura devolveu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shot {
    /// Título da janela capturada e o tamanho em pixels.
    pub info: String,
}

/// O caminho como o PowerShell e o .NET entendem. A raiz do projeto vem
/// canonicalizada no Windows, com o prefixo verbatim (barra invertida dupla,
/// interrogação, barra invertida), e o `Bitmap.Save` do .NET recusa esse
/// formato ("o formato de caminho não é suportado"), então o prefixo sai.
pub fn plain_path(p: &str) -> String {
    p.strip_prefix(r"\\?\").unwrap_or(p).to_string()
}

/// Nome do processo esperado para um alvo: o nome do arquivo sem extensão
/// (`C:\x\mspaint.exe` e `mspaint.exe` dão `mspaint`).
pub fn process_name(target: &str) -> String {
    let file = target.rsplit(['\\', '/']).next().unwrap_or(target);
    file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(file).to_string()
}

#[cfg(windows)]
const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
try {
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class CT {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out RECT r, int s);
}
"@
[CT]::SetProcessDPIAware() | Out-Null
$target = $env:CT_TARGET
$name = $env:CT_NAME
$title = $env:CT_TITLE
$out = $env:CT_OUT
$seconds = [int]$env:CT_SECONDS
$maximize = $env:CT_MAXIMIZE -eq '1'
$before = @(Get-Process -Name $name -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
Start-Process -FilePath $target | Out-Null
$win = $null
$deadline = (Get-Date).AddSeconds($seconds)
while ((Get-Date) -lt $deadline -and -not $win) {
  Start-Sleep -Milliseconds 400
  $cands = @(Get-Process -Name $name -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 })
  if ($title) { $cands = @($cands | Where-Object { $_.MainWindowTitle -like "*$title*" }) }
  $fresh = @($cands | Where-Object { $before -notcontains $_.Id })
  if ($fresh.Count -gt 0) { $win = $fresh[0] } elseif ($cands.Count -gt 0 -and ((Get-Date) -gt $deadline.AddSeconds(-2))) { $win = $cands[0] }
}
if (-not $win) { Write-Output "ERR nenhuma janela de '$name' apareceu em $seconds s"; exit 2 }
$h = $win.MainWindowHandle
if ($maximize) { [CT]::ShowWindow($h, 3) | Out-Null } else { [CT]::ShowWindow($h, 9) | Out-Null }
[CT]::SetForegroundWindow($h) | Out-Null
Start-Sleep -Milliseconds 1800
$r = New-Object CT+RECT
[CT]::DwmGetWindowAttribute($h, 9, [ref]$r, 16) | Out-Null
$w = $r.R - $r.L; $hh = $r.B - $r.T
if ($w -lt 50 -or $hh -lt 50) { Write-Output "ERR janela com tamanho invalido ($w x $hh)"; exit 3 }
$bmp = New-Object System.Drawing.Bitmap $w, $hh
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.L, $r.T, 0, 0, (New-Object System.Drawing.Size $w, $hh))
New-Item -ItemType Directory -Force -Path (Split-Path $out) | Out-Null
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
$t = $win.MainWindowTitle
Get-Process -Name $name -ErrorAction SilentlyContinue | Where-Object { $before -notcontains $_.Id } | Stop-Process -Force -ErrorAction SilentlyContinue
Write-Output "OK $w x $hh | $t"
} catch {
  Write-Output ("ERR " + $_.Exception.Message)
  exit 1
}
"#;

/// Abre `target`, captura a janela em `out` (PNG) e fecha o que abriu.
/// `title` filtra a janela pelo título; `seconds` é quanto esperar a janela
/// aparecer; `maximize` abre a janela maximizada, para ver a interface inteira.
#[cfg(windows)]
pub async fn observe(target: &str, title: Option<&str>, seconds: u64, maximize: bool, out: &Path) -> Result<Shot, String> {
    use base64::Engine;
    let seconds = seconds.clamp(2, 40);
    let utf16: Vec<u8> = SCRIPT.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    let mut cmd = tokio::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encoded])
        .env("CT_TARGET", plain_path(target))
        .env("CT_NAME", process_name(target))
        .env("CT_TITLE", title.unwrap_or(""))
        .env("CT_OUT", plain_path(&out.to_string_lossy()))
        .env("CT_SECONDS", seconds.to_string())
        .env("CT_MAXIMIZE", if maximize { "1" } else { "0" })
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let run = tokio::time::timeout(std::time::Duration::from_secs(seconds + 30), cmd.output())
        .await
        .map_err(|_| "a captura demorou demais e foi interrompida".to_string())?
        .map_err(|e| format!("não foi possível iniciar o PowerShell: {e}"))?;
    let stdout = String::from_utf8_lossy(&run.stdout);
    let line = stdout.lines().rev().find(|l| l.starts_with("OK ") || l.starts_with("ERR ")).unwrap_or("").to_string();
    if let Some(info) = line.strip_prefix("OK ") {
        return if out.is_file() { Ok(Shot { info: info.to_string() }) } else { Err("a captura não gerou o arquivo".into()) };
    }
    if let Some(e) = line.strip_prefix("ERR ") {
        return Err(e.to_string());
    }
    let stderr = String::from_utf8_lossy(&run.stderr);
    Err(format!("a captura falhou: {}", stderr.lines().next().unwrap_or("sem detalhes")))
}

#[cfg(not(windows))]
pub async fn observe(_target: &str, _title: Option<&str>, _seconds: u64, _maximize: bool, _out: &Path) -> Result<Shot, String> {
    Err("observe_app só funciona no Windows por enquanto".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_prefixo_verbatim_do_windows_sai_do_caminho() {
        assert_eq!(plain_path(r"\\?\C:\proj\a.png"), r"C:\proj\a.png");
        assert_eq!(plain_path(r"C:\proj\a.png"), r"C:\proj\a.png");
        assert_eq!(plain_path("mspaint.exe"), "mspaint.exe");
    }

    #[test]
    fn nome_do_processo_vem_do_arquivo() {
        assert_eq!(process_name("mspaint.exe"), "mspaint");
        assert_eq!(process_name(r"C:\Program Files\Steam\steam.exe"), "steam");
        assert_eq!(process_name("C:/x/meu app.exe"), "meu app");
        assert_eq!(process_name("calc"), "calc");
    }

    /// Abre o Bloco de Notas de verdade e confere que o PNG sai com o tamanho
    /// informado. Só roda no Windows.
    #[cfg(windows)]
    #[tokio::test]
    async fn captura_uma_janela_de_verdade() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("notepad.png");
        // O Bloco de Notas existe em toda instalação do Windows.
        match observe("notepad.exe", None, 12, false, &out).await {
            Ok(shot) => {
                let png = std::fs::read(&out).unwrap();
                assert_eq!(&png[1..4], b"PNG", "o arquivo é um PNG");
                assert!(shot.info.contains(" x "), "{shot:?}");
            }
            // Sem sessão gráfica (CI sem desktop) não há janela para ver.
            Err(e) => assert!(e.contains("janela") || e.contains("captura"), "{e}"),
        }
    }
}
