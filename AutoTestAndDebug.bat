@echo off
rem ---------------------------------------------------------------
rem AutoTestAndDebug.bat - Windows port of AutoTestAndDebug.sh
rem Design doc: docs/ Windows bat porting design (01-*.md under the Windows* folder)
rem
rem Purpose:
rem   Pick one available coding Agent CLI (claude / codex / opencode), load
rem   AutoTestAndDebug.windows.md (preferred) or AutoTestAndDebug.md as the
rem   prompt, and run the full one-session loop: git pull -> change sensing
rem   -> random knowledge-base selection -> laew -debug live test under
rem   TestWorkSpace/ -> tmpPlan/ report -> code fix -> cargo test + rebuild
rem   + e2e regression -> Chinese git commit/push -> cleanup. All permissions
rem   bypassed.
rem
rem Main differences vs the .sh (see design doc):
rem   - prompt candidates: AutoTestAndDebug.windows.md first, fallback .md
rem   - claude receives the prompt via stdin (cmd argv limit is 8191 chars)
rem   - flock -> best-effort directory lock (atomic md + pid.txt liveness)
rem   - nohup/setsid -> PowerShell Start-Process hidden cmd + wrapper script;
rem     the background process re-runs this very script (--bg-run internal arg)
rem   - tee -> PowerShell StreamWriter (live echo + UTF-8 log)
rem
rem Environment variables:
rem   AGENT_CLI        force agent selection (claude|codex|opencode)
rem   AGENT_SMOKE      =1 foreground smoke mode (call-chain unit test)
rem   CLAUDE_BIN / CODEX_BIN / OPENCODE_BIN   binary path overrides
rem   OPENCODE_MODEL   opencode model (provider/model format)
rem   LOG_RETAIN_DAYS  run-log retention days (default 30)
rem   PROMPT_FILE_OVERRIDE  prompt file override (debugging aid, wins over built-ins)
rem
rem Implementation rules (hard-won, read before editing):
rem   * Keep this file PURE ASCII + CRLF. cmd.exe mis-parses batch files that
rem     contain multi-byte characters (UTF-8 or GBK alike) once they get
rem     structurally complex; Chinese text belongs to the .md docs, not here.
rem     The Chinese guard phrase is built at runtime from [char] codes.
rem   * No goto (label seeking is unreliable in CJK files); branches are
rem     if/else nesting + call subroutines + two-line if to propagate rc.
rem   * No `exit /b N` inside a `for` block (exit code gets lost); use a flag
rem     variable and exit at top level after the loop.
rem   * Inside blocks, reference dynamic values via delayed expansion !VAR!
rem     (%VAR% expands at block parse time).
rem ---------------------------------------------------------------
setlocal EnableExtensions EnableDelayedExpansion

rem ---------- work dir = agent launch dir = script dir (callable from anywhere) ----------
set "PROJECT_DIR=%~dp0"
if "%PROJECT_DIR:~-1%"=="\" set "PROJECT_DIR=%PROJECT_DIR:~0,-1%"
set "LOG_DIR=%PROJECT_DIR%\logs"
set "SCRIPT_TAG=AutoTestAndDebug"

rem ---------- background worker branch (re-entered via hidden process) ----------
rem two-line if: the second line reads !ERRORLEVEL! AFTER the call returned
if /i "%~1"=="--bg-run" call :bg_run
if /i "%~1"=="--bg-run" exit /b !ERRORLEVEL!

rem ---------- timestamp ----------
set "TS="
for /f "delims=" %%t in ('powershell -NoProfile -Command "Get-Date -Format yyyyMMdd_HHmmss"') do set "TS=%%t"
if not defined TS set "TS=00000000_000000"

if not exist "%LOG_DIR%" mkdir "%LOG_DIR%"

rem ---------- binary paths (overridable via env) ----------
if not defined CLAUDE_BIN set "CLAUDE_BIN=claude"
if not defined CODEX_BIN set "CODEX_BIN=codex"
if not defined OPENCODE_BIN set "OPENCODE_BIN=opencode"

rem ---------- purge expired logs ----------
call :cleanup_old_logs

rem ---------- locate prompt file (windows variant preferred) ----------
set "PROMPT_FILE="
if defined PROMPT_FILE_OVERRIDE if exist "%PROMPT_FILE_OVERRIDE%" set "PROMPT_FILE=%PROMPT_FILE_OVERRIDE%"
if not defined PROMPT_FILE if exist "%PROJECT_DIR%\AutoTestAndDebug.windows.md" set "PROMPT_FILE=%PROJECT_DIR%\AutoTestAndDebug.windows.md"
if not defined PROMPT_FILE if exist "%PROJECT_DIR%\AutoTestAndDebug.md" set "PROMPT_FILE=%PROJECT_DIR%\AutoTestAndDebug.md"
if not defined PROMPT_FILE if exist "%CD%\AutoTestAndDebug.windows.md" set "PROMPT_FILE=%CD%\AutoTestAndDebug.windows.md"
if not defined PROMPT_FILE if exist "%CD%\AutoTestAndDebug.md" set "PROMPT_FILE=%CD%\AutoTestAndDebug.md"
if not defined PROMPT_FILE (
  echo [%SCRIPT_TAG%] [ERROR] prompt file AutoTestAndDebug.windows.md / AutoTestAndDebug.md not found in script dir or cwd.
  call :append_run_index "script=%SCRIPT_TAG%" "agent=none" "event=error_prompt_missing" "exit=1"
  exit /b 1
)

rem ---------- guard clause check (Chinese phrase built from char codes) ----------
rem pattern = [char]0x7EDD 0x5BF9 0x7981 0x6B62 0x4FEE 0x6539 0x20 0x60 CLAUDE.md 0x60 0x3001 0x60 AGENTS.md 0x60
powershell -NoProfile -Command "$p = [string]::Concat([char]0x7EDD,[char]0x5BF9,[char]0x7981,[char]0x6B62,[char]0x4FEE,[char]0x6539,[char]0x20,[char]0x60,'CLAUDE.md',[char]0x60,[char]0x3001,[char]0x60,'AGENTS.md',[char]0x60); if (Select-String -LiteralPath '%PROMPT_FILE%' -Pattern $p -SimpleMatch -Quiet -Encoding UTF8) { exit 0 } else { exit 1 }"
if errorlevel 1 (
  echo [%SCRIPT_TAG%] [ERROR] %PROMPT_FILE% is missing the guard clause - must forbid editing CLAUDE.md/AGENTS.md. Refusing to start.
  echo [%SCRIPT_TAG%] [ERROR] fix %PROMPT_FILE% first, then re-run this script.
  call :append_run_index "script=%SCRIPT_TAG%" "agent=none" "event=error_prompt_guard" "exit=2"
  exit /b 2
)

rem ---------- project layout precheck ----------
rem (no exit inside else-blocks: cmd loses the exit code there; flag + top-level exit)
set "LAYOUT_OK=1"
if not exist "%PROJECT_DIR%\Cargo.toml" set "LAYOUT_OK="
if not exist "%PROJECT_DIR%\src" set "LAYOUT_OK="
if not defined LAYOUT_OK (
  echo [%SCRIPT_TAG%] [ERROR] %PROJECT_DIR% has no Cargo.toml or src/ - not the laew project root.
  call :append_run_index "script=%SCRIPT_TAG%" "agent=none" "event=error_project_layout" "exit=4"
  exit /b 4
)
cd /d "%PROJECT_DIR%"

rem ---------- pick agent: AGENT_CLI forced > random among available ----------
set "AVAIL="
set /a AVAIL_N=0
for %%a in (claude codex opencode) do (
  call :agent_bin_of %%a AGENT_BIN_TMP
  where "!AGENT_BIN_TMP!" >nul 2>nul
  if not errorlevel 1 (
    set "AVAIL=!AVAIL! %%a"
    set /a AVAIL_N+=1
  )
)
if !AVAIL_N! equ 0 (
  echo [%SCRIPT_TAG%] [ERROR] no usable Agent CLI found - candidates: claude codex opencode. Refusing to start.
  call :append_run_index "script=%SCRIPT_TAG%" "agent=none" "event=error_no_agent" "exit=3"
  exit /b 3
)
set "SELECTED_AGENT="
set "CLI_BAD="
if defined AGENT_CLI (
  set "AC_TMP=%TEMP%\laew_avail_%RANDOM%.tmp"
  > "!AC_TMP!" echo !AVAIL!
  findstr /r /c:"\<%AGENT_CLI%\>" "!AC_TMP!" >nul
  if errorlevel 1 set "CLI_BAD=1"
  del "!AC_TMP!" >nul 2>&1
) else (
  set /a PICK_IDX=!RANDOM! %% !AVAIL_N! + 1
  set /a PICK_K=0
  for %%a in (!AVAIL!) do (
    set /a PICK_K+=1
    if !PICK_K! equ !PICK_IDX! set "SELECTED_AGENT=%%a"
  )
  echo [%SCRIPT_TAG%] random pick agent : !SELECTED_AGENT! - available:!AVAIL! -
)
if defined CLI_BAD (
  echo [%SCRIPT_TAG%] [ERROR] AGENT_CLI=%AGENT_CLI% not available; available:!AVAIL!
  exit /b 3
)
if defined AGENT_CLI if not defined CLI_BAD (
  set "SELECTED_AGENT=%AGENT_CLI%"
  echo [%SCRIPT_TAG%] AGENT_CLI forced agent : !SELECTED_AGENT!
)

rem ---------- AGENT_SMOKE=1 foreground smoke mode ----------
if "%AGENT_SMOKE%"=="1" call :smoke_run
if "%AGENT_SMOKE%"=="1" exit /b !ERRORLEVEL!

rem ================================================================
rem full mode: launch the one-shot agent session detached
rem ================================================================
set "LOG_FILE=%LOG_DIR%\auto_testdebug_!SELECTED_AGENT!_%TS%.log"
call :print_section_header "%SCRIPT_TAG%" "%PROMPT_FILE%" "!LOG_FILE!" "%PROJECT_DIR%" "!SELECTED_AGENT!"
for %%F in ("!LOG_FILE!") do set "LOG_BASE=%%~nxF"
call :append_run_index "script=%SCRIPT_TAG%" "agent=!SELECTED_AGENT!" "event=start" "log=logs/!LOG_BASE!"

rem ---------- build the background wrapper script (redirect + self-delete) ----------
set "WRAPPER=%LOG_DIR%\bg_wrapper_%TS%.cmd"
> "%WRAPPER%" echo @echo off
>> "%WRAPPER%" echo call "%~f0" --bg-run ^>^> "!LOG_FILE!" 2^>^&1
>> "%WRAPPER%" echo del "%%~f0" ^>nul 2^>^&1

rem ---------- pass env vars to the background child (inherited) ----------
set "LAEW_BG_AGENT=!SELECTED_AGENT!"
set "LAEW_BG_PROMPT=%PROMPT_FILE%"
set "LAEW_BG_LOG=!LOG_FILE!"

rem ---------- launch detached hidden cmd process, capture PID ----------
set "BG_PID="
for /f "usebackq delims=" %%p in (`powershell -NoProfile -Command "$p = Start-Process -FilePath 'cmd.exe' -ArgumentList '/c','%WRAPPER%' -WindowStyle Hidden -PassThru; $p.Id"`) do set "BG_PID=%%p"
call :append_run_index "script=%SCRIPT_TAG%" "agent=!SELECTED_AGENT!" "event=launched" "pid=!BG_PID!"

echo [%SCRIPT_TAG%] agent [!SELECTED_AGENT!] launched in background - PID !BG_PID!
echo [%SCRIPT_TAG%] log       : !LOG_FILE!
echo [%SCRIPT_TAG%] run index : %LOG_DIR%\auto_run_index.log
echo [%SCRIPT_TAG%] the agent follows the prompt: sync - pick - live test - tmpPlan report - fix - regression - Chinese commit/push.
echo [%SCRIPT_TAG%] caller is not blocked and may continue other work.
echo [%SCRIPT_TAG%] hint: AGENT_SMOKE=1 for foreground smoke test;
echo [%SCRIPT_TAG%]       AGENT_CLI=claude/codex/opencode to force the agent.
exit /b 0

rem ================================================================
rem subroutines
rem ================================================================

rem ---------- agent_bin_of <agent> <outvar>: agent name -> binary name ----------
:agent_bin_of

if "%~1"=="claude"   ( set "%~2=%CLAUDE_BIN%"   & exit /b 0 )
if "%~1"=="codex"    ( set "%~2=%CODEX_BIN%"    & exit /b 0 )
if "%~1"=="opencode" ( set "%~2=%OPENCODE_BIN%" & exit /b 0 )
set "%~2="
exit /b 0

rem ---------- cleanup_old_logs: purge run logs past retention (index kept forever) ----------
:cleanup_old_logs

set "COL_RETAIN=%LOG_RETAIN_DAYS%"
if not defined COL_RETAIN set "COL_RETAIN=30"
for %%m in ("*_testdebug_*.log" "*_test_*.log" "*_debug_*.log") do (
  forfiles /P "%LOG_DIR%" /M %%~m /D -%COL_RETAIN% /C "cmd /c del @path" >nul 2>&1
)
rem stale background wrappers older than 1 day (running workers are minutes old)
forfiles /P "%LOG_DIR%" /M "bg_wrapper_*.cmd" /D -1 /C "cmd /c del @path" >nul 2>&1
exit /b 0

rem ---------- append_run_index <key=value> ...: append one line to run index ----------
:append_run_index

set "ARI="
for %%k in (%*) do set "ARI=!ARI! %%~k"
>> "%LOG_DIR%\auto_run_index.log" echo [%DATE% %TIME%]!ARI!
exit /b 0

rem ---------- bg_log <tag> <message>: timestamped log ----------
rem foreground: dual write (console + LOG_FILE); background: console only
rem (output already redirected into the log by the wrapper script)
:bg_log

if defined LAEW_BG_LOG (
  echo [%DATE% %TIME%] [%~1] %~2
) else (
  echo [%DATE% %TIME%] [%~1] %~2
  >> "%LOG_FILE%" echo [%DATE% %TIME%] [%~1] %~2
)
exit /b 0

rem ---------- print_section_header <tag> <prompt> <log> <workdir> <agent> ----------
:print_section_header

set "PSH_TAG=%~1"
set "PSH_PROMPT=%~2"
set "PSH_LOG=%~3"
set "PSH_DIR=%~4"
set "PSH_AGENT=%~5"
set "PSH_GIT=unknown"
for /f "delims=" %%h in ('git -C "%PSH_DIR%" rev-parse --short HEAD 2^>nul') do set "PSH_GIT=%%h"
set "PSH_AVAIL="
for %%a in (claude codex opencode) do (
  call :agent_bin_of %%a PSH_BIN
  where "!PSH_BIN!" >nul 2>nul
  if not errorlevel 1 set "PSH_AVAIL=!PSH_AVAIL! %%a"
)
call :agent_bin_of "%PSH_AGENT%" PSH_ABIN
call :resolve_bin "!PSH_ABIN!"
set "PSH_ABIN_PATH=!RB_OUT!"
if not defined PSH_ABIN_PATH set "PSH_ABIN_PATH=NOT FOUND"
set "PSH_WINVER=unknown"
for /f "delims=" %%v in ('ver 2^>nul') do set "PSH_WINVER=%%v"
(
  echo ============================================================
  echo [!PSH_TAG!] start time    : %DATE% %TIME%
  echo [!PSH_TAG!] work dir      : !PSH_DIR!
  echo [!PSH_TAG!] prompt file   : !PSH_PROMPT!
  echo [!PSH_TAG!] log file      : !PSH_LOG!
  echo [!PSH_TAG!] selected agent: !PSH_AGENT!
  echo [!PSH_TAG!] agent binary  : !PSH_ABIN_PATH!
  echo [!PSH_TAG!] agents avail  : !PSH_AVAIL!
  echo [!PSH_TAG!] git head      : !PSH_GIT!
  echo [!PSH_TAG!] os version    : !PSH_WINVER!
  echo ============================================================
) >> "%PSH_LOG%"
exit /b 0

rem ---------- resolve_bin <name>: full path with extension (prefer .cmd/.exe/.bat) -> RB_OUT ----------
:resolve_bin

set "RB_OUT="
for /f "delims=" %%b in ('where "%~1" 2^>nul') do (
  if not defined RB_OUT set "RB_OUT=%%b"
  set "RB_EXT=%%~xb"
  if /i "!RB_EXT!"==".cmd" set "RB_OUT=%%b"
  if /i "!RB_EXT!"==".exe" set "RB_OUT=%%b"
  if /i "!RB_EXT!"==".bat" set "RB_OUT=%%b"
)
exit /b 0

rem ---------- opencode_model_arg: derive provider/model -> OC_MODEL ----------
:opencode_model_arg

set "OC_MODEL="
if defined OPENCODE_MODEL (
  set "OC_MODEL=%OPENCODE_MODEL%"
) else (
  set "OC_CFG=%USERPROFILE%\.config\opencode\config.json"
  if exist "!OC_CFG!" (
    where node >nul 2>nul
    if not errorlevel 1 (
      for /f "usebackq delims=" %%m in (`node -e "try{const c=require(process.argv[1]);process.stdout.write(c.model||'')}catch(e){}" "!OC_CFG!"`) do set "OC_MODEL=%%m"
    )
  )
)
if not defined OC_MODEL set "OC_MODEL=liusm191-server-model/liusm191-server-model"
if "!OC_MODEL:/=!"=="!OC_MODEL!" set "OC_MODEL=!OC_MODEL!/!OC_MODEL!"
exit /b 0

rem ---------- invoke_agent <agent> <prompt_file> <workdir> <log_file|""> ----------
rem Run the agent with permissions bypassed, prompt fed via stdin.
rem cmd argv limit is 8191 chars -> all three agents read the prompt from stdin.
rem The temp exec script holds a static pipeline command, dodging the
rem "pipe char from variable expansion is not re-parsed" trap.
:invoke_agent

set "IA_AGENT=%~1"
set "IA_PROMPT=%~2"
set "IA_DIR=%~3"
set "IA_LOG=%~4"
if not exist "%IA_PROMPT%" (
  echo [%SCRIPT_TAG%] [agent_cli] [ERROR] prompt file not found: %IA_PROMPT%
  exit /b 64
)
set "IA_BIN="
set "IA_ARGS="
if /i "%IA_AGENT%"=="claude" (
  call :resolve_bin "%CLAUDE_BIN%"
  set "IA_BIN=!RB_OUT!"
  set "IA_ARGS=--dangerously-skip-permissions -p"
)
if /i "%IA_AGENT%"=="codex" (
  call :resolve_bin "%CODEX_BIN%"
  set "IA_BIN=!RB_OUT!"
  set "IA_ARGS=exec --dangerously-bypass-approvals-and-sandbox -"
)
if /i "%IA_AGENT%"=="opencode" (
  call :resolve_bin "%OPENCODE_BIN%"
  set "IA_BIN=!RB_OUT!"
  call :opencode_model_arg
  echo [%SCRIPT_TAG%] [agent_cli] opencode model arg: !OC_MODEL!
  set "IA_ARGS=run --auto -m !OC_MODEL!"
)
if not defined IA_BIN (
  echo [%SCRIPT_TAG%] [agent_cli] [ERROR] unknown agent or binary unavailable: %IA_AGENT%
  exit /b 64
)
set "IA_EXEC=%TEMP%\laew_exec_%RANDOM%%RANDOM%.cmd"
> "%IA_EXEC%" echo @echo off
>> "%IA_EXEC%" echo pushd "%IA_DIR%"
>> "%IA_EXEC%" echo type "%IA_PROMPT%" ^| "!IA_BIN!" !IA_ARGS!
>> "%IA_EXEC%" echo set "RC=%%ERRORLEVEL%%"
>> "%IA_EXEC%" echo popd
>> "%IA_EXEC%" echo exit /b %%RC%%
set "IA_RC=0"
if not "%IA_LOG%"=="" (
  powershell -NoProfile -Command "[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false; $w = New-Object System.IO.StreamWriter('%IA_LOG%', $true, (New-Object System.Text.UTF8Encoding $false)); cmd /c '%IA_EXEC%' | ForEach-Object { $w.WriteLine([string]$_); $_ }; $w.Close(); exit $LASTEXITCODE"
  set "IA_RC=!ERRORLEVEL!"
) else (
  call "%IA_EXEC%"
  set "IA_RC=!ERRORLEVEL!"
)
del "%IA_EXEC%" >nul 2>&1
exit /b %IA_RC%

rem ---------- smoke_run: foreground smoke (tiny prompt, verifies script -> agent chain) ----------
:smoke_run

set "LOG_FILE=%LOG_DIR%\smoke_testdebug_!SELECTED_AGENT!_%TS%.log"
call :print_section_header "%SCRIPT_TAG%-smoke" "%PROMPT_FILE%" "!LOG_FILE!" "%PROJECT_DIR%" "!SELECTED_AGENT!"
for %%F in ("!LOG_FILE!") do set "LOG_BASE=%%~nxF"
call :append_run_index "script=%SCRIPT_TAG%" "agent=!SELECTED_AGENT!" "event=smoke_start" "log=logs/!LOG_BASE!"

set "SMOKE_FILE=%TEMP%\laew_testdebug_smoke_prompt_%RANDOM%%RANDOM%.md"
> "%SMOKE_FILE%" echo This is an Agent call-chain smoke test - AutoTestAndDebug.bat AGENT_SMOKE mode.
>> "%SMOKE_FILE%" echo Do exactly one thing: reply a single line "SMOKE_OK" followed by your current working directory.
>> "%SMOKE_FILE%" echo Do not read files, do not run any command, do nothing else.

call :bg_log "%SCRIPT_TAG%-smoke" "smoke mode: foreground run of !SELECTED_AGENT!, prompt: %SMOKE_FILE%"
call :invoke_agent "!SELECTED_AGENT!" "%SMOKE_FILE%" "%PROJECT_DIR%" "!LOG_FILE!"
set "AGENT_RC=%ERRORLEVEL%"
del "%SMOKE_FILE%" >nul 2>&1
call :bg_log "%SCRIPT_TAG%-smoke" "!SELECTED_AGENT! exit code : %AGENT_RC%"
if "%AGENT_RC%"=="0" (
  call :bg_log "%SCRIPT_TAG%-smoke" "smoke passed - call chain OK"
  call :append_run_index "script=%SCRIPT_TAG%" "agent=!SELECTED_AGENT!" "event=smoke_done" "exit=0"
) else (
  call :bg_log "%SCRIPT_TAG%-smoke" "smoke FAILED - exit !AGENT_RC! - check log above"
  call :append_run_index "script=%SCRIPT_TAG%" "agent=!SELECTED_AGENT!" "event=smoke_done" "exit=!AGENT_RC!"
)
exit /b %AGENT_RC%

rem ---------- bg_run: background worker (stdout/stderr already redirected to run log) ----------
:bg_run

if not defined LAEW_BG_AGENT exit /b 5
if not defined LAEW_BG_PROMPT exit /b 5
if not exist "%LOG_DIR%" mkdir "%LOG_DIR%"
if not defined CLAUDE_BIN set "CLAUDE_BIN=claude"
if not defined CODEX_BIN set "CODEX_BIN=codex"
if not defined OPENCODE_BIN set "OPENCODE_BIN=opencode"
cd /d "%PROJECT_DIR%"

rem ---------- re-entry lock (atomic md + pid liveness probe) ----------
set "LOCK_DIR=%LOG_DIR%\auto_testdebug.lock"
md "%LOCK_DIR%" >nul 2>&1
if errorlevel 1 (
  rem lock dir exists -> probe holder pid liveness
  set "LOCK_PID="
  if exist "%LOCK_DIR%\pid.txt" set /p LOCK_PID=<"%LOCK_DIR%\pid.txt"
  set "LOCK_BUSY=0"
  if defined LOCK_PID (
    for /f "usebackq tokens=2 delims=," %%t in (`tasklist /FI "PID eq !LOCK_PID!" /FO CSV /NH 2^>nul`) do (
      set "LOCK_TOK=%%t"
      if "!LOCK_TOK:~1,-1!"=="!LOCK_PID!" set "LOCK_BUSY=1"
    )
  )
  if "!LOCK_BUSY!"=="1" (
    call :bg_log "%SCRIPT_TAG%" "another instance is running - PID !LOCK_PID! - exiting re-entry guard."
    call :append_run_index "script=%SCRIPT_TAG%" "agent=%LAEW_BG_AGENT%" "event=skip_locked" "exit=0"
    exit /b 0
  )
  rem stale lock -> purge and re-acquire
  rd /s /q "%LOCK_DIR%" >nul 2>&1
  md "%LOCK_DIR%" >nul 2>&1
  if errorlevel 1 (
    call :bg_log "%SCRIPT_TAG%" "cannot purge stale lock, exiting re-entry guard."
    exit /b 0
  )
)
set "WORKER_PID="
for /f "usebackq delims=" %%p in (`powershell -NoProfile -Command "(Get-CimInstance Win32_Process -Filter ('ProcessId=' + $PID)).ParentProcessId"`) do set "WORKER_PID=%%p"
if defined WORKER_PID (
  > "%LOCK_DIR%\pid.txt" echo !WORKER_PID!
)

rem ---------- best-effort git pull (60s watchdog; failure only logs) ----------
call :bg_log "%SCRIPT_TAG%" "pulling latest main - git pull --ff-only - ..."
powershell -NoProfile -Command "$p = Start-Process -FilePath git -ArgumentList 'pull','--ff-only','origin','main' -NoNewWindow -PassThru -RedirectStandardOutput \"$env:TEMP\laew_pull_out.txt\" -RedirectStandardError \"$env:TEMP\laew_pull_err.txt\"; if (-not $p.WaitForExit(60000)) { try { $p.Kill() } catch {}; Write-Output 'git pull timed out after 60s - killed, best-effort'; exit 124 }; Get-Content \"$env:TEMP\laew_pull_out.txt\"; Get-Content \"$env:TEMP\laew_pull_err.txt\"; Remove-Item \"$env:TEMP\laew_pull_out.txt\",\"$env:TEMP\laew_pull_err.txt\" -ErrorAction SilentlyContinue; exit $p.ExitCode"
if errorlevel 1 (
  call :bg_log "%SCRIPT_TAG%" "git pull failed/diverged - non-fatal; the agent session handles conflicts per prompt."
) else (
  call :bg_log "%SCRIPT_TAG%" "git pull done or already up to date."
)

rem ---------- run the one-shot agent ----------
call :bg_log "%SCRIPT_TAG%" "starting %LAEW_BG_AGENT%, prompt file: %LAEW_BG_PROMPT%"
call :invoke_agent "%LAEW_BG_AGENT%" "%LAEW_BG_PROMPT%" "%PROJECT_DIR%" ""
set "AGENT_RC=%ERRORLEVEL%"
call :bg_log "%SCRIPT_TAG%" "%LAEW_BG_AGENT% exit code : %AGENT_RC%"
call :append_run_index "script=%SCRIPT_TAG%" "agent=%LAEW_BG_AGENT%" "event=agent_done" "exit=%AGENT_RC%"
call :bg_log "%SCRIPT_TAG%" "full run finished"
call :append_run_index "script=%SCRIPT_TAG%" "agent=%LAEW_BG_AGENT%" "event=done" "exit=%AGENT_RC%"

rd /s /q "%LOCK_DIR%" >nul 2>&1
exit /b 0
