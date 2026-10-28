# 构建两个 Windows 分发包：
#   - lite：只含 exe，使用系统 Edge/Chrome
#   - bundled：额外包含 chrome-headless-shell（约 200MB）
#
# 用法（在 Windows 上运行）：powershell -ExecutionPolicy Bypass -File scripts\package.ps1 [-Mode all|lite|bundled]
param(
    [ValidateSet("all", "lite", "bundled")]
    [string]$Mode = "all"
)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

$versionLine = Select-String -Path "Cargo.toml" -Pattern '^version\s*=\s*"(.+)"' |
    Select-Object -First 1
if (-not $versionLine) { throw "无法从 Cargo.toml 读取版本号" }
$version = $versionLine.Matches[0].Groups[1].Value

Write-Host "构建 release（v$version）..."
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build 失败" }

$bin = "target\release\cmcc-campus-auth.exe"
$dist = "dist"
if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
New-Item -ItemType Directory -Path $dist | Out-Null

$common = @("README.md", "LICENSE", "THIRD_PARTY_NOTICES.md")

if ($Mode -eq "all" -or $Mode -eq "lite") {
    $name = "cmcc-campus-auth-$version-win64-lite"
    $dir = Join-Path $dist $name
    New-Item -ItemType Directory -Path $dir | Out-Null
    Copy-Item $bin $dir
    foreach ($f in $common) { Copy-Item $f $dir }
    New-Item -ItemType Directory -Path (Join-Path $dir "docs") -Force | Out-Null
    Copy-Item "docs\screenshot.png" (Join-Path $dir "docs")
    Compress-Archive -Path $dir -DestinationPath (Join-Path $dist "$name.zip") -Force
    Write-Host "已生成：$dist\$name.zip"
}

if ($Mode -eq "all" -or $Mode -eq "bundled") {
    $name = "cmcc-campus-auth-$version-win64-bundled"
    $dir = Join-Path $dist $name
    New-Item -ItemType Directory -Path $dir | Out-Null
    Copy-Item $bin $dir
    foreach ($f in $common) { Copy-Item $f $dir }
    New-Item -ItemType Directory -Path (Join-Path $dir "docs") -Force | Out-Null
    Copy-Item "docs\screenshot.png" (Join-Path $dir "docs")
    & (Join-Path $PSScriptRoot "fetch_chromium.ps1") -Dest (Join-Path $dir "chromium")
    Compress-Archive -Path $dir -DestinationPath (Join-Path $dist "$name.zip") -Force
    Write-Host "已生成：$dist\$name.zip"
}
