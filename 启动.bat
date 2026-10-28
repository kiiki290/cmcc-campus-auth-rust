@echo off
chcp 936 >nul
cd /d "%~dp0"
if not exist "%~dp0scripts\run.ps1" (
    echo 找不到 scripts\run.ps1
    pause
    exit /b 1
)
where pwsh >nul 2>nul
if errorlevel 1 (
    echo 未找到 PowerShell 7（pwsh），请先安装后再运行。
    pause
    exit /b 1
)
echo 正在启动校园宽带自动认证...
pwsh -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\run.ps1" %*
if errorlevel 1 (
    echo.
    echo 启动失败，请查看上方错误信息。
    pause
)
