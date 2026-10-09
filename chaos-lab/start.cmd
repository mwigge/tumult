@echo off
cd /d "%~dp0"
docker compose up -d --build --wait --wait-timeout 120
if errorlevel 1 exit /b 1
echo Tumult Chaos Lab is ready at http://localhost:8089 (or LAB_PORT from .env).
start http://localhost:8089
