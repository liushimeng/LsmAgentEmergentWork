@echo off
rem ---------------------------------------------------------------
rem rebuild_restart_app.bat - Windows port of rebuild_restart_app.sh
rem Design doc: docs/ Windows bat porting design (01-*.md under the Windows* folder)
rem
rem Usage:
rem   rebuild_restart_app.bat            release build (auto force-rebuild on version mismatch)
rem   rebuild_restart_app.bat --debug    debug build
rem   rebuild_restart_app.bat --force    release build, always touch build.rs
rem   rebuild_restart_app.bat --debug --force
rem
rem Output:
rem   laew.exe                 binary (overwrite; old one renamed first to bypass run-lock)
rem   testReport\build.log     build log (UTF-8)
rem
rem Implementation rules (hard-won, read before editing):
rem   * Keep this file PURE ASCII + CRLF. cmd.exe mis-parses batch files that
rem     contain multi-byte characters (UTF-8 or GBK alike) once they get
rem     structurally complex; Chinese text belongs to the .md docs, not here.
rem   * No `exit /b N` inside a `for` block (exit code gets lost); use a flag
rem     variable and exit at top level after the loop.
rem   * Inside blocks, reference dynamic values via delayed expansion !VAR!
rem   * Inside for /f command strings, escape literal "=" as ^= (cmd eats the
rem     rest of the token otherwise; e.g. rev-parse --short^=8).
rem     (%VAR% expands at block parse time).
rem ---------------------------------------------------------------
setlocal EnableExtensions EnableDelayedExpansion

rem ---------- root dir = script dir ----------
set "ROOT_DIR=%~dp0"
if "%ROOT_DIR:~-1%"=="\" set "ROOT_DIR=%ROOT_DIR:~0,-1%"
cd /d "%ROOT_DIR%"

set "PROFILE=release"
set "CARGO_ARGS=--release"
set "FORCE=0"
set "HELP_REQ="
set "FATAL="
for %%a in (%*) do (
  call :parse_one %%a
  if errorlevel 2 ( set "FATAL=2" & set "PARSE_STOP=1" )
  if defined HELP_REQ set "PARSE_STOP=1"
)
if defined FATAL exit /b %FATAL%
if defined HELP_REQ call :print_usage
if defined HELP_REQ exit /b 0

echo [rebuild] root dir : %ROOT_DIR%
echo [rebuild] profile  : %PROFILE%
if "%FORCE%"=="1" echo [rebuild] force    : --force - touch build.rs to defeat cargo cache

if not exist "testReport" mkdir "testReport"
set "LOG_FILE=testReport\build.log"

rem ---------- 1) --force: touch build.rs to defeat cargo incremental cache ----------
if "%FORCE%"=="1" (
  echo [rebuild] --force: touch build.rs
  copy /b "build.rs"+,, >nul
)

rem ---------- 2) build (live console output + log file) ----------
echo [rebuild] cargo build %CARGO_ARGS%  log: %LOG_FILE%
set "BUILD_APPEND=0"
call :build_logged
if errorlevel 1 (
  echo [rebuild] ERROR: cargo build failed - exit !ERRORLEVEL! - see %LOG_FILE%
  exit /b 1
)

rem ---------- 3) install binary into project root ----------
call :install_bin
if errorlevel 1 exit /b 1

rem ---------- 4) version check: guard against stale cargo fingerprint cache ----------
call :git_hash
set "EXPECT=%GH%"
call :bin_hash
set "ACTUAL=%BH%"
if not "%ACTUAL%"=="%EXPECT%" (
  echo [rebuild] WARN: version mismatch: laew.exe reports git !ACTUAL! - empty means none - HEAD is %EXPECT%
  echo [rebuild] force rebuild via touch build.rs
  copy /b "build.rs"+,, >nul
  set "BUILD_APPEND=1"
  call :build_logged
  if errorlevel 1 (
    echo [rebuild] ERROR: forced rebuild failed - exit !ERRORLEVEL!
    exit /b 1
  )
  call :install_bin
  if errorlevel 1 exit /b 1
  call :bin_hash
  if not "!BH!"=="%EXPECT%" (
    echo [rebuild] ERROR: still git !BH! after forced rebuild [expected %EXPECT%]
    echo [rebuild] check: git status / system clock / target dir; try cargo clean
    exit /b 1
  )
)

rem ---------- 5) remote HEAD compare (PS 5s watchdog fetch, best-effort) ----------
call :remote_hash
if not "%RH%"=="unknown" if not "%RH%"=="%EXPECT%" (
  rem when local commits are not pushed yet, remote lags behind HEAD - do not
  rem misreport "remote ahead"; only warn when origin/main is not an ancestor.
  git cat-file -e "%RH%^{commit}" >nul 2>nul
  if errorlevel 1 (
    echo [rebuild] WARN: remote origin/main %RH% is ahead of HEAD %EXPECT%
    echo [rebuild] WARN: binary was built from local HEAD; consider: git pull
  ) else (
    git merge-base --is-ancestor "%RH%" "%EXPECT%" >nul 2>nul
    if errorlevel 1 (
      echo [rebuild] WARN: remote origin/main %RH% is ahead of HEAD %EXPECT%
      echo [rebuild] WARN: binary was built from local HEAD; consider: git pull
    ) else (
      echo [rebuild] remote origin/main %RH% is contained in HEAD %EXPECT%, local commits pending push
    )
  )
)

rem ---------- 6) show artifact version and current commit ----------
for /f "delims=" %%l in ('git log -1 --format="%%h %%ci %%s" 2^>nul') do echo [rebuild] commit: %%l
echo [rebuild] artifact version:
"%ROOT_DIR%\laew.exe" --version

rem ---------- 7) PATH resolution hint ----------
set "PATH_HIT="
for /f "delims=" %%p in ('where laew 2^>nul') do if not defined PATH_HIT set "PATH_HIT=%%p"
if defined PATH_HIT if /i not "%PATH_HIT%"=="%ROOT_DIR%\laew.exe" (
  echo.
  echo [rebuild] WARN: ============================================================
  echo [rebuild] WARN: PATH resolves laew to: !PATH_HIT!
  echo [rebuild] WARN: which differs from the fresh build: %ROOT_DIR%\laew.exe
  echo [rebuild] WARN: typing 'laew' may hit an older binary
  echo [rebuild] WARN: use absolute path '%ROOT_DIR%\laew.exe' or fix PATH
  echo [rebuild] WARN: ============================================================
)

echo [rebuild] output: %ROOT_DIR%\laew.exe
echo [rebuild] done
exit /b 0

rem ================================================================
rem subroutines
rem ================================================================

rem ---------- parse_one <arg>: parse a single command line option ----------
:parse_one

if /i "%~1"=="--debug" ( set "PROFILE=debug" & set "CARGO_ARGS=" & exit /b 0 )
if /i "%~1"=="--force" ( set "FORCE=1" & exit /b 0 )
if /i "%~1"=="--help" ( set "HELP_REQ=1" & exit /b 0 )
if /i "%~1"=="-h" ( set "HELP_REQ=1" & exit /b 0 )
echo [rebuild] unknown option: %~1
exit /b 2

rem ---------- print_usage: print usage after HELP_REQ detected ----------
:print_usage

echo Usage: rebuild_restart_app.bat [--debug] [--force]
echo   release build by default; --force touches build.rs to force rebuild
exit /b 0

rem ---------- build_logged: cargo build, live echo + UTF-8 log ----------
rem env deps: BUILD_APPEND (0=overwrite / 1=append), PROFILE, LOG_FILE
:build_logged

powershell -NoProfile -Command "$a = ($env:BUILD_APPEND -eq '1'); [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false; $w = New-Object System.IO.StreamWriter('%LOG_FILE%', $a, (New-Object System.Text.UTF8Encoding $false)); cmd /c 'cargo build %CARGO_ARGS% 2>&1' | ForEach-Object { $w.WriteLine([string]$_); $_ }; $w.Close(); exit $LASTEXITCODE"
exit /b %ERRORLEVEL%

rem ---------- install_bin: copy artifact to root (rename old first) ----------
:install_bin

set "IB_BIN=target\%PROFILE%\laew.exe"
if not exist "%IB_BIN%" (
  echo [rebuild] ERROR: artifact not found at %IB_BIN%
  exit /b 1
)
rem detect running laew processes (warn only; a running exe can be renamed but not overwritten)
tasklist /FI "IMAGENAME eq laew.exe" 2>nul | findstr /I "laew.exe" >nul && (
  echo [rebuild] WARN: running laew.exe processes detected:
  for /f "skip=1 delims=" %%l in ('tasklist /FI "IMAGENAME eq laew.exe" 2^>nul') do echo [rebuild]   %%l
  echo [rebuild] WARN: about to replace laew.exe; running instances keep the old binary
)
if exist "%ROOT_DIR%\laew.exe" (
  move /y "%ROOT_DIR%\laew.exe" "%ROOT_DIR%\laew.old.exe" >nul 2>&1 || echo [rebuild] WARN: cannot rename old laew.exe, trying direct overwrite
)
copy /y "%IB_BIN%" "%ROOT_DIR%\laew.exe" >nul
if errorlevel 1 (
  echo [rebuild] ERROR: copy failed - laew.exe may be locked; stop running instances and retry -
  exit /b 1
)
del "%ROOT_DIR%\laew.old.exe" >nul 2>&1
exit /b 0

rem ---------- git_hash: current HEAD short hash (8 chars, aligned with build.rs) ----------
:git_hash

set "GH="
for /f "delims=" %%h in ('git rev-parse --short^=8 HEAD 2^>nul') do set "GH=%%h"
if not defined GH set "GH=unknown"
exit /b 0

rem ---------- bin_hash: extract git hash from laew.exe --version ----------
rem version line looks like: 0.1.2 (build 2026-09-12 08:00:00, git 3e6c8018)
:bin_hash

set "BH="
for /f "usebackq delims=" %%h in (`powershell -NoProfile -Command "$v = & '%ROOT_DIR%\laew.exe' --version 2>$null; if ($v -match 'git ([0-9a-zA-Z]+)\)') { $Matches[1] }"`) do set "BH=%%h"
exit /b 0

rem ---------- remote_hash: 5s watchdog fetch, then origin/main short hash ----------
:remote_hash

set "RH="
powershell -NoProfile -Command "$p = Start-Process -FilePath git -ArgumentList 'fetch','--no-tags','origin','main' -WindowStyle Hidden -PassThru; if (-not $p.WaitForExit(5000)) { try { $p.Kill() } catch {} }" >nul 2>&1
for /f "delims=" %%h in ('git rev-parse --short^=8 origin/main 2^>nul') do set "RH=%%h"
if not defined RH set "RH=unknown"
exit /b 0
