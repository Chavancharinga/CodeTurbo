# Instala o CodeTurbo no Windows. No PowerShell:
#   irm https://raw.githubusercontent.com/Chavancharinga/CodeTurbo/main/install.ps1 | iex
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$repo = 'Chavancharinga/CodeTurbo'
$target = 'x86_64-pc-windows-msvc'
$dir = if ($env:CODETURBO_INSTALL_DIR) { $env:CODETURBO_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\CodeTurbo' }
$url = "https://github.com/$repo/releases/latest/download/codeturbo-$target.tar.gz"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ('codeturbo-' + [guid]::NewGuid())

New-Item -ItemType Directory -Force -Path $tmp, $dir | Out-Null
try {
    Write-Host 'Baixando o CodeTurbo...'
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri $url -OutFile (Join-Path $tmp 'codeturbo.tar.gz') -UseBasicParsing
    tar -xzf (Join-Path $tmp 'codeturbo.tar.gz') -C $tmp
    if ($LASTEXITCODE -ne 0) { throw 'Falha ao extrair o pacote do CodeTurbo.' }
    Copy-Item (Join-Path $tmp 'codeturbo.exe') (Join-Path $dir 'codeturbo.exe') -Force
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not $userPath) { $userPath = '' }
if (($userPath -split ';') -notcontains $dir) {
    $newPath = if ($userPath) { $userPath.TrimEnd(';') + ";$dir" } else { $dir }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    $env:Path = "$env:Path;$dir"
}

Write-Host "CodeTurbo instalado em $dir"
Write-Host 'Para começar, abra um novo terminal e execute: codeturbo'
