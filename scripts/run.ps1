#requires -Version 7.0
<#
.SYNOPSIS
    构建（如需）并启动校园宽带自动认证 GUI。

.DESCRIPTION
    默认使用 release 构建；源码、Cargo.toml、Cargo.lock 或 assets 比 exe 新时自动重新构建。
    程序已在运行时，单实例机制会把现有窗口唤到前台，不会重复启动。

.PARAMETER DebugBuild
    使用 debug 构建（编译快、运行慢），便于开发调试。
    注意不能命名为 -Debug：与 PowerShell 公共参数冲突。

.PARAMETER NoBuild
    跳过构建，直接启动现有 exe；exe 不存在时报错。

.PARAMETER ForceBuild
    无条件重新构建。

.PARAMETER AppArgs
    透传给程序的参数，例如 --screenshot C:\temp\app.png。

.EXAMPLE
    .\scripts\run.ps1

.EXAMPLE
    .\scripts\run.ps1 -DebugBuild

.EXAMPLE
    .\scripts\run.ps1 --screenshot C:\temp\app.png
#>
param(
    [switch]$DebugBuild,
    [switch]$NoBuild,
    [switch]$ForceBuild,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$AppArgs
)

$ErrorActionPreference = "Stop"
if ($NoBuild -and $ForceBuild) { throw "-NoBuild 与 -ForceBuild 不能同时使用" }

Set-Location (Split-Path $PSScriptRoot -Parent)

$profileName = if ($DebugBuild) { "debug" } else { "release" }
$exe = Join-Path (Get-Location).Path "target\$profileName\cmcc-campus-auth.exe"

function Resolve-Cargo {
    $cmd = Get-Command cargo -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $fallback = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
    if (Test-Path $fallback) { return $fallback }
    throw "未找到 cargo，请先安装 Rust 工具链"
}

$needBuild = $ForceBuild -or -not (Test-Path $exe)
if (-not $needBuild -and -not $NoBuild) {
    $stamp = (Get-Item $exe).LastWriteTimeUtc
    $inputs = @(
        Get-ChildItem "src" -Recurse -File -Filter *.rs -ErrorAction SilentlyContinue
        Get-Item "Cargo.toml", "Cargo.lock" -ErrorAction SilentlyContinue
        Get-ChildItem "assets" -Recurse -File -ErrorAction SilentlyContinue
    )
    if ($inputs | Where-Object { $_.LastWriteTimeUtc -gt $stamp }) {
        $needBuild = $true
        Write-Host "检测到源码更新，需要重新构建"
    }
}

if ($needBuild -and -not $NoBuild) {
    $cargo = Resolve-Cargo
    Write-Host "构建 $profileName（首次 release 构建较慢）..."
    $cargoArgs = @("build")
    if (-not $DebugBuild) { $cargoArgs += "--release" }
    & $cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo build 失败" }
}

if (-not (Test-Path $exe)) { throw "找不到 $exe，去掉 -NoBuild 重新构建" }

$procArgs = @{
    FilePath         = $exe
    WorkingDirectory = (Get-Location).Path
}
if ($AppArgs) { $procArgs.ArgumentList = $AppArgs }

$p = Start-Process @procArgs -PassThru
Write-Host "已启动（PID $($p.Id)）：$exe"
