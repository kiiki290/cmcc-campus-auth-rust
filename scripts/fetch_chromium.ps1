# 下载 Chrome for Testing 的 chrome-headless-shell 到指定目录（Windows）。
# 用法：scripts\fetch_chromium.ps1 -Dest <目标目录>
param(
    [Parameter(Mandatory = $true)]
    [string]$Dest
)

$ErrorActionPreference = "Stop"

$manifestUrl = "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json"
$manifest = Invoke-RestMethod -Uri $manifestUrl
$entry = $manifest.channels.Stable.downloads.'chrome-headless-shell' |
    Where-Object { $_.platform -eq "win64" } |
    Select-Object -First 1
if (-not $entry) { throw "清单中没有 win64 的 chrome-headless-shell" }

$cacheRoot = if ($env:CMCC_CHROMIUM_CACHE) {
    $env:CMCC_CHROMIUM_CACHE
} else {
    Join-Path $env:LOCALAPPDATA "cmcc-campus-auth\chrome-for-testing"
}
New-Item -ItemType Directory -Path $cacheRoot -Force | Out-Null
$cachedZip = Join-Path $cacheRoot ("chrome-headless-shell-" + $manifest.channels.Stable.version + "-win64.zip")

$temp = Join-Path $env:TEMP ("chromium-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $temp | Out-Null
try {
    if (-not (Test-Path $cachedZip) -or (Get-Item $cachedZip).Length -eq 0) {
        Write-Host "下载：$($entry.url)"
        Invoke-WebRequest -Uri $entry.url -OutFile "$cachedZip.part"
        Move-Item -Force "$cachedZip.part" $cachedZip
    } else {
        Write-Host "使用缓存：$cachedZip"
    }
    Expand-Archive -Path $cachedZip -DestinationPath $temp -Force

    $dirName = [System.IO.Path]::GetFileNameWithoutExtension(
        ([System.Uri]$entry.url).AbsolutePath
    )
    $src = Join-Path $temp $dirName
    if (-not (Test-Path $src)) { throw "压缩包结构异常：缺少 $dirName" }

    New-Item -ItemType Directory -Path $Dest -Force | Out-Null
    $target = Join-Path $Dest $dirName
    if (Test-Path $target) { Remove-Item -Recurse -Force $target }
    Copy-Item -Recurse $src $target
    Write-Host "已放置：$(Join-Path $target 'chrome-headless-shell.exe')"
}
finally {
    Remove-Item -Recurse -Force $temp -ErrorAction SilentlyContinue
}
