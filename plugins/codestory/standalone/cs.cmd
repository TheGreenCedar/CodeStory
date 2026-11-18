@echo off
rem Managed by codestory-cli setup.
setlocal DisableDelayedExpansion
if /i "%~1"=="find" goto lookup
if /i "%~1"=="refs" goto lookup
if /i "%~1"=="read" goto lookup
echo Usage: cs find TERM... / cs refs NAME... / cs read SELECTOR...
exit /b 2
:lookup
for /f "delims=" %%G in ('git rev-parse --show-toplevel 2^>nul') do cd /d "%%G"
@CODESTORY_CLI@ %*
