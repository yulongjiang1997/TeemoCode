@echo off
set PATH=C:\Users\12090\.cargo\bin;C:\Users\12090\msys64\mingw64\bin;%PATH%
cd /d D:\works\ziji\MonkeyCode\desktop
echo ===[1/3] beforeBuildCommand: npm ci + vite build(uidist)===
cd ui-next
call npm ci
if errorlevel 1 ( echo FRONTEND-CI-FAILED & exit /b 1 )
call npm run build
if errorlevel 1 ( echo FRONTEND-BUILD-FAILED & exit /b 1 )
cd ..
echo ===[2/3] cargo tauri build release (bundle.windows.conf.json)===
npx --yes @tauri-apps/cli@^2 build --config bundle.windows.conf.json
if errorlevel 1 ( echo TAURI-BUILD-FAILED & exit /b 1 )
echo ===[3/3] DONE===
dir /b /s target\release\bundle\nsis\*.exe 2>nul
