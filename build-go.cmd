@echo off
setlocal
set LOG=D:\Qoderwork\build-log.txt
set PATH=C:\Users\Administrator\.workbuddy\binaries\node\versions\22.22.2-6;C:\Users\Administrator\.cargo\bin;%PATH%
echo ==== build start %date% %time% ==== > "%LOG%"
where node >> "%LOG%" 2>&1
where cargo >> "%LOG%" 2>&1

echo [1/4] stopping running app...
taskkill /IM archive-gallery.exe /F >> "%LOG%" 2>&1

echo [2/4] frontend build...
cd /d D:\Qoderwork
call npm.cmd run build >> "%LOG%" 2>&1
if errorlevel 1 goto fail

echo [3/4] cargo build, 20-40 min, keep window open...
cd /d D:\Qoderwork\src-tauri
call cargo.exe build --profile fastdev --bin archive-gallery >> "%LOG%" 2>&1
if errorlevel 1 goto fail

echo [4/4] install exe...
powershell -NoProfile -Command "Copy-Item -LiteralPath 'D:\Qoderwork\src-tauri\target\fastdev\archive-gallery.exe' -Destination (Join-Path $env:LOCALAPPDATA ([string][char]0x753B + [char]0x5323 + '\archive-gallery.exe')) -Force"
if errorlevel 1 goto fail

echo.
echo ====================================
echo   DONE - open desktop Huaxia now
echo ====================================
goto end

:fail
echo.
echo FAILED - see D:\Qoderwork\build-log.txt

:end
echo.
pause
endlocal
