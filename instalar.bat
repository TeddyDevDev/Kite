@echo off
chcp 65001 >nul
title Instalar Kite
cd /d "%~dp0"

echo.
echo  ==============================
echo     KITE - instalacion
echo  ==============================
echo.

rem Cerrar Kite si esta abierto (si no, Windows no deja reemplazar el .exe)
taskkill /im Kite.exe /f >nul 2>&1

echo  [1/3] Compilando la version final (la primera vez tarda unos minutos)...
cargo build --release
if errorlevel 1 (
    echo.
    echo  ERROR: no se pudo compilar. Copia el mensaje de arriba y pasaselo a Claude.
    pause
    exit /b 1
)

echo  [2/3] Copiando Kite.exe...
copy /y "target\release\kite.exe" "Kite.exe" >nul

echo  [3/3] Creando accesos directos (escritorio y menu Inicio)...
powershell -NoProfile -ExecutionPolicy Bypass -Command ^
  "$ws = New-Object -ComObject WScript.Shell;" ^
  "foreach ($dir in @([Environment]::GetFolderPath('Desktop'), [Environment]::GetFolderPath('Programs'))) {" ^
  "  $s = $ws.CreateShortcut((Join-Path $dir 'Kite.lnk'));" ^
  "  $s.TargetPath = '%~dp0Kite.exe';" ^
  "  $s.WorkingDirectory = '%~dp0';" ^
  "  $s.IconLocation = '%~dp0Kite.exe,0';" ^
  "  $s.Description = 'Kite - ayudante de League of Legends';" ^
  "  $s.Save() }"

echo.
echo  Listo. Ya puedes abrir Kite desde el escritorio o el menu Inicio.
echo  Cuando haya cambios nuevos, vuelve a ejecutar este archivo.
echo.
start "" "%~dp0Kite.exe"
timeout /t 4 >nul
