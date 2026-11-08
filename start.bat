@echo off
REM Adom Fusion 360 Bridge Server launcher
REM Usage: start.bat [--port 8773]

cd /d "%~dp0"
python server.py %*
pause
