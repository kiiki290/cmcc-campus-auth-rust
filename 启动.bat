@echo off
rem 保存当前控制台代码页，退出前还原，避免在已有终端里运行时污染宿主编码
for /f "tokens=2 delims=:" %%a in ('chcp') do set "OLDCP=%%a"
set "OLDCP=%OLDCP: =%"
if not "%OLDCP%"=="936" chcp 936 >nul
cd /d "%~dp0"
if not exist "%~dp0scripts\run.ps1" (
    echo 找不到 scripts\run.ps1
    goto :fail
)
where pwsh >nul 2>nul
if errorlevel 1 (
    echo 未找到 PowerShell 7（pwsh），请先安装后再运行。
    goto :fail
)
echo 正在启动校园宽带自动认证...
pwsh -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\run.ps1" %*
if errorlevel 1 (
    echo.
    echo 启动失败，请查看上方错误信息。
    goto :fail
)
if defined OLDCP chcp %OLDCP% >nul
exit /b 0

:fail
if defined OLDCP chcp %OLDCP% >nul
pause
exit /b 1
