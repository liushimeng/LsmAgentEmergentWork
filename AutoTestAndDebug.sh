#!/usr/bin/env bash
# AutoTestAndDebug.sh
# ---------------------------------------------------------------
# 用途：
#   laew(LsmAgentEmergentWork) 自动化测试与 Debug 一体化入口。随机选择一个
#   可用的编程 Agent CLI（claude / codex / opencode），读取脚本同目录的
#   AutoTestAndDebug.md 作为提示词，一次会话内执行「git 拉取最新 main →
#   近 10 天变更感知 → 知识库随机选条(10~100 条,优先 bash/编程) → TestWorkSpace/
#   下 laew -debug 实测 Agent 能力 → 问题汇总落盘 tmpPlan/ 方案 → 按方案修码
#   → cargo test + rebuild + e2e 回归 → git 中文提交推送(冲突自解) → 清理
#   中间产物」全流程闭环；不再产出 testReport/ 问题报告，全程 bypass 权限。
#
# 与旧脚本的关系（2026-09-12 重构合并，旧脚本已删除）：
#   AutoTestAndSaveReport —— 只测不改产报告   ─┐
#   AutoDebugTestReport   —— 消费报告做修复   ─┴─ 合并为本脚本（一体化闭环）
#   设计文档：docs/自动化测试与Debug一体化/01-设计与解决方案.md
#
# 特性：
#   - 工作目录与 Agent 启动目录均为脚本所在目录（可在任意路径调用）
#   - 支持三种编程 Agent CLI（claude / codex / opencode）随机选择；
#     AGENT_CLI 环境变量可强制指定某个 Agent（定向调试用）
#   - AGENT_SMOKE=1 冒烟模式：前台同步运行微型提示词，只验证调用链路，
#     不跑真实测试、不改文件、不 git 提交（供「每个 Agent 单元测试」用）
#   - 提示词守门：AutoTestAndDebug.md 必须显式声明「绝对禁止修改
#     CLAUDE.md/AGENTS.md」，否则视为提示词被误改，拒绝启动（exit 2）
#   - 启动前预检：工程基本结构（Cargo.toml + src/）缺失时直接退出
#   - flock 防重入：同一时刻只允许一个一体化实例（锁由 Agent 所在进程持有）
#   - 后台段 best-effort git pull --ff-only（超时 60s，失败仅记日志不阻塞；
#     完整的拉取/冲突处理/提交/推送由 Agent 按提示词 §0/§6 执行，脚本不插手）
#   - 完整模式通过 nohup + setsid + & + disown 脱离调用者，不阻塞
#   - 日志体系：
#     * 单次运行日志 logs/auto_testdebug_<Agent名>_<时间戳>.log
#       （启动头记录时间/目录/提示词/Agent 二进制/Git HEAD/可用候选池，
#        正文为 git 拉取 + Agent 全程 stdout/stderr，关键节点带时间戳）
#     * 运行索引日志 logs/auto_run_index.log：每次运行追加 key=value 单行
#       （脚本名/Agent 名/事件/时间/PID/退出码），便于事后 grep 审计
#     * 旧运行日志超过保留期（缺省 30 天，LOG_RETAIN_DAYS 可覆盖）自动清理
#       （兼容清理旧 AutoTestAndSaveReport/AutoDebugTestReport 遗留日志）
#
# 环境变量：
#   AGENT_CLI        强制指定 Agent（claude|codex|opencode）
#   AGENT_SMOKE      置 1 启用前台冒烟模式（单元测试调用链路）
#   CLAUDE_BIN / CODEX_BIN / OPENCODE_BIN   各二进制路径覆盖
#   OPENCODE_MODEL   opencode 模型（provider/model 格式），缺省自动从
#                    ~/.config/opencode/config.json 推导
#   LOG_RETAIN_DAYS  运行日志保留天数（缺省 30）
# ---------------------------------------------------------------

set -u

# ---------- 配置 ----------
PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_DIR="${PROJECT_DIR}/logs"
SCRIPT_TAG="AutoTestAndDebug"
PROMPT_FILE_NAME="AutoTestAndDebug.md"
TS="$(date +%Y%m%d_%H%M%S)"
LOCK_FILE="${LOG_DIR}/auto_testdebug.lock"

mkdir -p "${LOG_DIR}"

# ---------- 二进制路径（可被环境变量覆盖） ----------
CLAUDE_BIN="${CLAUDE_BIN:-claude}"
CODEX_BIN="${CODEX_BIN:-codex}"
OPENCODE_BIN="${OPENCODE_BIN:-opencode}"

# 候选 Agent 全集（顺序无关，随机选取）
AGENT_CANDIDATES=(claude codex opencode)

# SELECTED_AGENT 由 pick_agent 填充
SELECTED_AGENT=""

# agent_binary_of <agent> —— 返回对应二进制名
agent_binary_of() {
    case "$1" in
        claude)   echo "${CLAUDE_BIN}" ;;
        codex)    echo "${CODEX_BIN}" ;;
        opencode) echo "${OPENCODE_BIN}" ;;
        *)        echo "" ;;
    esac
}

# list_available_agents —— 输出 PATH 中可用的 Agent 名（每行一个）
list_available_agents() {
    local a bin
    for a in "${AGENT_CANDIDATES[@]}"; do
        bin="$(agent_binary_of "${a}")"
        if command -v "${bin}" >/dev/null 2>&1; then
            echo "${a}"
        fi
    done
}

# pick_agent <caller_tag> —— 选择一个可用 Agent，结果写入 SELECTED_AGENT。
# 选择优先级：AGENT_CLI 环境变量强制指定 > 从全部可用 Agent 中随机选择。
# 无可用 Agent 时直接退出（exit 3）。
pick_agent() {
    local caller_tag="${1:-AutoAgent}"
    local available=()
    local a
    while IFS= read -r a; do
        [[ -n "${a}" ]] && available+=("${a}")
    done < <(list_available_agents)

    if [[ ${#available[@]} -eq 0 ]]; then
        echo "[${caller_tag}] [ERROR] 未发现任何可用 Agent CLI（候选: ${AGENT_CANDIDATES[*]}），拒绝启动。" >&2
        exit 3
    fi

    if [[ -n "${AGENT_CLI:-}" ]]; then
        local found=""
        for a in "${available[@]}"; do
            if [[ "${a}" == "${AGENT_CLI}" ]]; then found="${a}"; break; fi
        done
        if [[ -z "${found}" ]]; then
            echo "[${caller_tag}] [ERROR] AGENT_CLI=${AGENT_CLI} 不可用；当前可用: ${available[*]}" >&2
            exit 3
        fi
        SELECTED_AGENT="${found}"
        echo "[${caller_tag}] AGENT_CLI 强制指定 Agent : ${SELECTED_AGENT}"
    else
        local idx=$(( RANDOM % ${#available[@]} ))
        SELECTED_AGENT="${available[$idx]}"
        echo "[${caller_tag}] 随机选中 Agent : ${SELECTED_AGENT}（可用候选: ${available[*]}）"
    fi
}

# opencode_model_arg —— 推导 opencode 的 provider/model 参数。
# 实测：opencode run 仅凭全局 config 的 "model" 字段（无 provider 前缀）会报
# ProviderModelNotFoundError，必须显式 -m；本机 provider 与模型同名时补全为
# provider/model 格式。
opencode_model_arg() {
    if [[ -n "${OPENCODE_MODEL:-}" ]]; then
        echo "${OPENCODE_MODEL}"
        return 0
    fi
    local cfg="${HOME}/.config/opencode/config.json"
    local m=""
    if [[ -f "${cfg}" ]] && command -v node >/dev/null 2>&1; then
        m="$(node -e 'try{const c=require(process.argv[1]);process.stdout.write(c.model||"")}catch(e){}' "${cfg}" 2>/dev/null || true)"
    fi
    if [[ -z "${m}" ]]; then
        m="liusm191-server-model/liusm191-server-model"
    elif [[ "${m}" != */* ]]; then
        # 仅有模型名时，补全为 provider/model（本工程 provider 与模型同名）
        m="${m}/${m}"
    fi
    echo "${m}"
}

# run_agent_with_prompt <agent> <prompt_file> [project_dir]
# 以「放开权限、全程自动化」方式运行指定 Agent，提示词取自 prompt 文件。
# 返回 Agent 进程的退出码。
# 各 Agent 非交互调用方式（2026-09-08 本机实测）：
#   codex    0.153.4   codex exec --dangerously-bypass-approvals-and-sandbox -
#                       （exec=非交互；`-` 从 stdin 读提示词，规避 argv 转义风险）
#   claude   2.1.263   claude --dangerously-skip-permissions -p "<prompt>"
#   opencode 1.18.27   opencode run --auto -m <provider/model> "<prompt>"
run_agent_with_prompt() {
    local agent="$1"
    local prompt_file="$2"
    local project_dir="${3:-${PROJECT_DIR:-$(pwd)}}"
    local prompt_text

    if [[ ! -f "${prompt_file}" ]]; then
        echo "[agent_cli] [ERROR] 提示词文件不存在: ${prompt_file}" >&2
        return 64
    fi

    case "${agent}" in
        claude)
            prompt_text="$(cat "${prompt_file}")"
            (cd "${project_dir}" && "${CLAUDE_BIN}" --dangerously-skip-permissions -p "${prompt_text}")
            ;;
        codex)
            # stdin 直读提示词文件，安全传递超长中文文本
            (cd "${project_dir}" && "${CODEX_BIN}" exec \
                --dangerously-bypass-approvals-and-sandbox \
                - < "${prompt_file}")
            ;;
        opencode)
            local model_arg
            model_arg="$(opencode_model_arg)"
            prompt_text="$(cat "${prompt_file}")"
            echo "[agent_cli] opencode 模型参数: ${model_arg}"
            (cd "${project_dir}" && "${OPENCODE_BIN}" run --auto -m "${model_arg}" "${prompt_text}")
            ;;
        *)
            echo "[agent_cli] [ERROR] 未知 Agent: ${agent}" >&2
            return 64
            ;;
    esac
}

# ---------- 统一日志工具 ----------

# bg_log <tag> <message...>
# 后台段日志助手：后台 shell 的 stdout/stderr 已重定向到运行日志文件，
# 此处仅补统一时间戳前缀。
bg_log() {
    local tag="$1"
    shift
    echo "[$(date '+%F %T')] [${tag}] $*"
}

# append_run_index <key=value> ...
# 运行索引日志：向 logs/auto_run_index.log 追加一行
#   [YYYY-MM-DD HH:MM:SS] key=value key=value ...
# 推荐键：script / agent / event(start|launched|agent_done|skip_locked|
#         smoke_start|smoke_done|done|error_*) / pid / exit / log
append_run_index() {
    local idx_file="${LOG_DIR}/auto_run_index.log"
    local line="[$(date '+%F %T')]"
    local kv
    for kv in "$@"; do
        line+=" ${kv}"
    done
    printf '%s\n' "${line}" >> "${idx_file}" 2>/dev/null || true
}

# cleanup_old_logs [retain_days]
# 清理 logs/ 下超过保留期的运行日志（本脚本 *_testdebug_* 与旧脚本遗留的
# *_test_* / *_debug_*）。
#   - 运行索引 auto_run_index.log 永久保留（审计用）。
#   - 保留期缺省 30 天，环境变量 LOG_RETAIN_DAYS 可覆盖。
cleanup_old_logs() {
    local retain_days="${1:-${LOG_RETAIN_DAYS:-30}}"
    [[ -d "${LOG_DIR}" ]] || return 0
    find "${LOG_DIR}" -maxdepth 1 -type f \
        \( -name '*_testdebug_*.log' -o -name '*_test_*.log' -o -name '*_debug_*.log' \) \
        -mtime +"${retain_days}" -delete 2>/dev/null || true
}

# print_section_header <tag> <prompt_file> <log_file> <workdir> <agent>
# 启动日志头：时间/目录/提示词/Agent 二进制/可用候选池/Git HEAD/Bash 版本，
# 便于把日志与代码版本对齐。
print_section_header() {
    local tag="$1"
    local prompt_file="$2"
    local log_file="$3"
    local workdir="$4"
    local agent="$5"
    local git_head avail_agents
    git_head="$(git -C "${workdir}" rev-parse --short HEAD 2>/dev/null || echo 'unknown')"
    avail_agents="$(list_available_agents 2>/dev/null | tr '\n' ' ')"
    {
        echo "============================================================"
        echo "[${tag}] 启动时间 : $(date '+%F %T')"
        echo "[${tag}] 工作目录 : ${workdir}"
        echo "[${tag}] 提示词文件: ${prompt_file}"
        echo "[${tag}] 日志文件  : ${log_file}"
        echo "[${tag}] 选中 Agent : ${agent}"
        echo "[${tag}] Agent 二进制: $(command -v "$(agent_binary_of "${agent}")" 2>/dev/null || echo 'NOT FOUND')"
        echo "[${tag}] 可用 Agents: ${avail_agents:-none}"
        echo "[${tag}] Git HEAD  : ${git_head}"
        echo "[${tag}] Bash 版本 : ${BASH_VERSION:-unknown}"
        echo "============================================================"
    } >> "${log_file}"
}

# ---------- locate_prompt_file <prompt_name> ----------
# 三层定位 prompt 文件:脚本目录绝对路径 → ./相对 → $PWD 相对。
# 命中后输出 readlink -f 的绝对路径；都不存在则 stderr 报错并返回非 0。
locate_prompt_file() {
    local prompt_name="$1"
    local candidate
    for candidate in \
        "${PROJECT_DIR}/${prompt_name}" \
        "./${prompt_name}" \
        "${PWD}/${prompt_name}"; do
        if [[ -f "${candidate}" ]]; then
            readlink -f "${candidate}"
            return 0
        fi
    done
    echo "[${SCRIPT_TAG}] [ERROR] 找不到 ${prompt_name},已检查:" >&2
    echo "  - ${PROJECT_DIR}/${prompt_name}" >&2
    echo "  - ./${prompt_name}" >&2
    echo "  - ${PWD}/${prompt_name}" >&2
    return 1
}

# ---------- start_agent_in_background <log_file> <bash_script> ----------
# nohup setsid bash -c "<script>" &; disown 封装。
#   log_file: stdout/stderr 重定向目标
#   bash_script: 传给 bash -c 的脚本内容
# 输出:启动 PID 到 stdout,失败退出码非 0。
start_agent_in_background() {
    local log_file="$1"
    local bash_script="$2"
    nohup setsid bash -c "${bash_script}" >> "${log_file}" 2>&1 </dev/null &
    local pid=$!
    disown "${pid}" 2>/dev/null || true
    echo "${pid}"
}

# ================================================================
# 主流程
# ================================================================

# ---------- 清理过期日志 ----------
cleanup_old_logs

# ---------- 定位提示词文件 ----------
PROMPT_FILE="$(locate_prompt_file "${PROMPT_FILE_NAME}")" || {
    append_run_index "script=${SCRIPT_TAG}" "agent=none" "event=error_prompt_missing" "exit=1"
    exit 1
}

# ---------- 规则文件守门（防止 prompt 被改回老版本/被误删条款）----------
# AutoTestAndDebug.md 必须显式声明「绝对禁止修改 CLAUDE.md/AGENTS.md」，
# 否则视为提示词被误改，提示人工修复并退出。
if ! grep -qF '绝对禁止修改 `CLAUDE.md`、`AGENTS.md`' "${PROMPT_FILE}" 2>/dev/null; then
    echo "[${SCRIPT_TAG}] [ERROR] ${PROMPT_FILE} 缺少「绝对禁止修改 CLAUDE.md/AGENTS.md」守门条款，拒绝启动。" >&2
    echo "[${SCRIPT_TAG}] [ERROR] 请先修复 ${PROMPT_FILE} 再重跑本脚本。" >&2
    append_run_index "script=${SCRIPT_TAG}" "agent=none" "event=error_prompt_guard" "exit=2"
    exit 2
fi

# ---------- 工程结构预检 ----------
if [[ ! -f "${PROJECT_DIR}/Cargo.toml" || ! -d "${PROJECT_DIR}/src" ]]; then
    echo "[${SCRIPT_TAG}] [ERROR] ${PROJECT_DIR} 缺少 Cargo.toml 或 src/，不是 laew 工程根目录。" >&2
    append_run_index "script=${SCRIPT_TAG}" "agent=none" "event=error_project_layout" "exit=4"
    exit 4
fi

cd "${PROJECT_DIR}" || { echo "[${SCRIPT_TAG}] [ERROR] 无法进入 ${PROJECT_DIR}" >&2; exit 1; }

# ---------- 选择 Agent ----------
pick_agent "${SCRIPT_TAG}"

# ---------- AGENT_SMOKE=1 冒烟模式（前台，单元测试调用链路） ----------
# 冒烟只验证「脚本 → Agent CLI」调用链路，跳过工程结构与 git 拉取。
if [[ "${AGENT_SMOKE:-0}" == "1" ]]; then
    LOG_FILE="${LOG_DIR}/smoke_testdebug_${SELECTED_AGENT}_${TS}.log"
    print_section_header "${SCRIPT_TAG}-smoke" "${PROMPT_FILE}" "${LOG_FILE}" "${PROJECT_DIR}" "${SELECTED_AGENT}"
    append_run_index "script=${SCRIPT_TAG}" "agent=${SELECTED_AGENT}" "event=smoke_start" "log=logs/$(basename "${LOG_FILE}")"

    # 微型提示词：只验证 Agent 能被拉起并正常应答，不跑真实测试
    SMOKE_FILE="$(mktemp /tmp/laew_testdebug_smoke_prompt_XXXXXX.md)"
    cat > "${SMOKE_FILE}" <<'SMOKE_EOF'
这是一次 Agent 调用链路冒烟测试（AutoTestAndDebug.sh AGENT_SMOKE 模式）。
请只做一件事：回复一行文本 "SMOKE_OK"，后跟你当前的工作目录路径。
不要读取文件、不要执行任何命令、不要做任何其他操作。
SMOKE_EOF

    bg_log "${SCRIPT_TAG}-smoke" "冒烟模式：前台运行 ${SELECTED_AGENT}，提示词: ${SMOKE_FILE}" | tee -a "${LOG_FILE}"
    run_agent_with_prompt "${SELECTED_AGENT}" "${SMOKE_FILE}" "${PROJECT_DIR}" 2>&1 | tee -a "${LOG_FILE}"
    AGENT_EXIT="${PIPESTATUS[0]}"
    rm -f "${SMOKE_FILE}"

    bg_log "${SCRIPT_TAG}-smoke" "${SELECTED_AGENT} 退出码 : ${AGENT_EXIT}" | tee -a "${LOG_FILE}"
    if [[ "${AGENT_EXIT}" -eq 0 ]]; then
        bg_log "${SCRIPT_TAG}-smoke" "冒烟通过 ✅（调用链路正常）" | tee -a "${LOG_FILE}"
        append_run_index "script=${SCRIPT_TAG}" "agent=${SELECTED_AGENT}" "event=smoke_done" "exit=0"
    else
        bg_log "${SCRIPT_TAG}-smoke" "冒烟失败 ❌（退出码 ${AGENT_EXIT}，请检查上方日志）" | tee -a "${LOG_FILE}"
        append_run_index "script=${SCRIPT_TAG}" "agent=${SELECTED_AGENT}" "event=smoke_done" "exit=${AGENT_EXIT}"
    fi
    exit "${AGENT_EXIT}"
fi

# ---------- 导出函数与变量供后台 bash -c 子 shell 复用 ----------
# start_agent_in_background 启动的是全新 bash 进程，不继承本脚本的函数与
# 普通变量；后台段需要 run_agent_with_prompt / bg_log / append_run_index
# 三个函数及它们依赖的变量，必须显式导出。
export CLAUDE_BIN CODEX_BIN OPENCODE_BIN OPENCODE_MODEL
export PROJECT_DIR LOG_DIR SCRIPT_TAG
export -f agent_binary_of list_available_agents run_agent_with_prompt \
    opencode_model_arg bg_log append_run_index 2>/dev/null || true

# ---------- 完整模式：日志文件名含 Agent 名 ----------
LOG_FILE="${LOG_DIR}/auto_testdebug_${SELECTED_AGENT}_${TS}.log"

# ---------- 启动日志头 + 运行索引 ----------
print_section_header "${SCRIPT_TAG}" "${PROMPT_FILE}" "${LOG_FILE}" "${PROJECT_DIR}" "${SELECTED_AGENT}"
append_run_index "script=${SCRIPT_TAG}" "agent=${SELECTED_AGENT}" "event=start" "log=logs/$(basename "${LOG_FILE}")"

# ---------- 启动（后台脱离，不阻塞调用者）----------
# 整个生命周期在同一个 bash -c 子 shell 中顺序执行：
#   1. flock 防重入（锁文件 logs/auto_testdebug.lock，Agent 退出自动释放）
#   2. best-effort git pull --ff-only（拉最新 main；失败仅记日志，
#      完整拉取/冲突处理由 Agent 按提示词 §0 执行）
#   3. 选中的 Agent CLI 读取提示词文件执行一体化全流程（放开权限，全程自动化）；
#      git 中文提交推送 / 产物清理由 Agent 按提示词 §6 自行完成，脚本不插手
# 注意：BG_SCRIPT 为双引号串，\$(...) 才是运行期求值，$(...) 会在构造期展开；
# 反引号禁止使用（构造期命令替换）。
BG_PID="$(start_agent_in_background "${LOG_FILE}" "
    cd '${PROJECT_DIR}'

    # ------- flock 防重入 -------
    exec 9>>'${LOCK_FILE}'
    if ! flock -n 9; then
        bg_log '${SCRIPT_TAG}' '已有一体化实例在运行，本次退出(防重入)。'
        append_run_index script=${SCRIPT_TAG} agent=${SELECTED_AGENT} event=skip_locked exit=0
        exit 0
    fi

    # ------- best-effort 拉取最新 main（不阻塞，冲突留给 Agent 处理） -------
    bg_log '${SCRIPT_TAG}' '尝试拉取最新 main (git pull --ff-only)...'
    if timeout 60 git pull --ff-only origin main >>'${LOG_FILE}' 2>&1; then
        bg_log '${SCRIPT_TAG}' 'git 拉取完成（或已最新）。'
    else
        bg_log '${SCRIPT_TAG}' 'git 拉取失败/分叉（不影响启动，由 Agent 会话 §0 处理冲突）。'
    fi

    # ------- 运行一体化 Agent -------
    bg_log '${SCRIPT_TAG}' '开始运行 ${SELECTED_AGENT}，提示词文件: ${PROMPT_FILE}'
    run_agent_with_prompt '${SELECTED_AGENT}' '${PROMPT_FILE}' '${PROJECT_DIR}'
    AGENT_EXIT=\$?
    bg_log '${SCRIPT_TAG}' '${SELECTED_AGENT} 退出码 : '\${AGENT_EXIT}
    append_run_index script=${SCRIPT_TAG} agent=${SELECTED_AGENT} event=agent_done exit=\"\${AGENT_EXIT}\" log=logs/$(basename "${LOG_FILE}")

    bg_log '${SCRIPT_TAG}' '全流程结束'
    append_run_index script=${SCRIPT_TAG} agent=${SELECTED_AGENT} event=done exit=\"\${AGENT_EXIT}\"
")"
append_run_index "script=${SCRIPT_TAG}" "agent=${SELECTED_AGENT}" "event=launched" "pid=${BG_PID}"

echo "[${SCRIPT_TAG}] 已后台启动 Agent [${SELECTED_AGENT}] (PID: ${BG_PID})"
echo "[${SCRIPT_TAG}] 日志 : ${LOG_FILE}"
echo "[${SCRIPT_TAG}] 运行索引日志 : ${LOG_DIR}/auto_run_index.log"
echo "[${SCRIPT_TAG}] Agent 按提示词执行 同步→选条→实测→tmpPlan方案→修码→回归→中文提交推送 全流程。"
echo "[${SCRIPT_TAG}] 调用者可继续执行其他操作，不会被阻塞。"
echo "[${SCRIPT_TAG}] 提示: AGENT_SMOKE=1 可前台冒烟验证调用链路；"
echo "[${SCRIPT_TAG}]       AGENT_CLI=<claude|codex|opencode> 可强制指定 Agent。"
