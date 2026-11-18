@echo off
rem Managed by codestory-cli setup.
setlocal DisableDelayedExpansion
if /i "%~1"=="find" goto lookup
if /i "%~1"=="refs" goto lookup
if /i "%~1"=="read" goto lookup
if /i "%~1"=="packet" goto lookup
echo Usage: cs find TERM... or --question QUESTION / cs refs TARGET... or --changed / cs read SELECTOR... / cs packet --question QUESTION
exit /b 2
:lookup
for /f "delims=" %%G in ('git rev-parse --show-toplevel 2^>nul') do cd /d "%%G"
if /i "%~1"=="packet" goto packet
@CODESTORY_CLI@ %*
exit /b %errorlevel%
:packet
@CODESTORY_CLI@ %* --launcher-read-only
