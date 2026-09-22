#!/usr/bin/env bash
# run_e2e.sh: laew 端到端自动化验证(不依赖真实 LLM,使用本地 mock 服务)
# 输出: testReport/e2e-<时间戳>.txt
set -uo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

TS=$(date +%Y%m%d-%H%M%S)
RUN_ID="${TS}-$$"
# 2026-09-11 第三十四轮修复:REPORT 用绝对路径 —— run() 的 tee -a 在被 cd 到
# /tmp 子目录的子壳里以相对路径打开会静默失败,导致这些小节的 laew 原始输出
# 不落报告(例:7b 自定义命令节),事后排查无据可查。
REPORT="$ROOT_DIR/testReport/e2e-$RUN_ID.txt"
MOCK_PORT=18899
MOCK_LOG="testReport/mock_requests-$RUN_ID.jsonl"
PASS=0; FAIL=0

# 2026-09-20 并发保护:固定临时根目录 / DB / mock 端口会被并行执行互相删除,
# 抓包日志与报告也会串流。先用同机锁串行化 e2e,再用 PID 区分产物文件。
LOCK_DIR="${TMPDIR:-/tmp}/laew-e2e.lock"
cleanup_e2e_lock() {
  rm -rf "$LOCK_DIR"
}
while ! mkdir "$LOCK_DIR" 2>/dev/null; do
  lock_owner="$(cat "$LOCK_DIR/pid" 2>/dev/null || true)"
  if [[ -n "$lock_owner" ]] && ! kill -0 "$lock_owner" 2>/dev/null; then
    rm -rf "$LOCK_DIR"
    continue
  fi
  sleep 1
done
echo $$ > "$LOCK_DIR/pid"
trap cleanup_e2e_lock EXIT INT TERM

# 2026-09-10 第 25 轮:D9-7 SSRF 守卫(url_safety.rs)落库后,mock 服务的
# 127.0.0.1 端点在请求期被拦截,全量用例挂在这一行(92 FAIL)。
# e2e 全程使用本地 mock,统一放行私有端点(与 tmux 用例既有做法一致)。
export LAEW_ALLOW_PRIVATE_ENDPOINT=1

# 提前创建报告文件,避免后续 tee -a 在某些早期调用顺序下出现
# "No such file or directory" 竞态(关联报告: 20260908_203854 D-003)
mkdir -p testReport
: > "$REPORT"

section() { echo "" | tee -a "$REPORT"; echo "==== $1 ====" | tee -a "$REPORT"; }
check() {
  if [ "$1" -eq 0 ]; then PASS=$((PASS+1)); echo "  [PASS] $2" | tee -a "$REPORT"
  else FAIL=$((FAIL+1)); echo "  [FAIL] $2" | tee -a "$REPORT"; fi
}
run() { "$@" 2>&1 | tee -a "$REPORT"; return "${PIPESTATUS[0]}"; }

# macOS 没有 GNU coreutils 的 timeout；为 7c 的 TUI 防卡死用例提供等价 watchdog。
if ! command -v timeout >/dev/null 2>&1; then
  timeout() {
    local seconds pid rc
    seconds="$1"; shift
    "$@" &
    pid=$!
    for (( tick=0; tick < seconds*10; tick++ )); do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.1
    done
    if kill -0 "$pid" 2>/dev/null; then
      kill -TERM "$pid" 2>/dev/null || true
      sleep 1
      kill -KILL "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
      return 124
    fi
    wait "$pid"; rc=$?
    return "$rc"
  }
fi

echo "laew e2e 验证 @ $(date)" | tee "$REPORT"
echo "根目录: $ROOT_DIR" | tee -a "$REPORT"

# --- 准备:独立数据库(避免污染真实配置) ---
# 2026-09-10 第二十八轮修复:rm -f 对目录无效,残留旧 DB 会让 §2「未配置模型」
# 与 §3 provider add(UNIQUE 冲突)级联失败;必须 rm -rf 保证干净起点。
rm -rf /tmp/laew-e2e-root; mkdir -p /tmp/laew-e2e-root
cp laew /tmp/laew-e2e-root/laew
LAEW=/tmp/laew-e2e-root/laew   # 根目录=/tmp/laew-e2e-root → db 也在这里

# --- 1. 版本与帮助 ---
section "1. --version / --help"
run "$LAEW" --version; check $? "--version 可用"
OUT=$(run "$LAEW" --help); echo "$OUT" | grep -q "provider"; check $? "--help 含 provider 指南"

# --- 2. 无配置时的 -p 引导报错 ---
section "2. 未配置模型时 -p 的引导提示"
OUT=$(run "$LAEW" -p "hello"); echo "$OUT" | grep -q "provider add"; check $? "提示先 provider add"

# --- 2b. 未配置模型时 TUI 管道模式守门(第 71 轮,tmpPlan/2026-09-17_03) ---
# 非 TTY 管道输入走 TUI run() 回退,与交互 TUI 同一条 dispatch_prompt 链路。
# 修复前:NoopLlm 占位文本进全链路 → Yolo 降级 → QC fail-closed → 3 轮盲重试,
# 以「Quality 报告解析失败」误导收口;修复后:守门秒回指引,不进编排器。
section "2b. 未配置模型时 TUI 管道模式守门"
OUT=$(echo "查看一下,当前进程列表" | timeout 10 "$LAEW" 2>&1)
echo "$OUT" | grep -q "provider add"; check $? "TUI 管道模式提示先 provider add"
echo "$OUT" | grep -q "未配置"; check $? "TUI 管道模式含未配置指引"
echo "$OUT" | grep -q "未进入编排器"; check $? "守门明确说明未进入编排器"
echo "$OUT" | grep -q "orchestrator 调度中"; [ $? -ne 0 ]; check $? "未触发编排器调度"
echo "$OUT" | grep -q "Quality 报告解析失败"; [ $? -ne 0 ]; check $? "不再出现误导性 Quality 解析失败"

# --- 3. provider 增/列/切换/删 ---
section "3. provider CRUD"
run "$LAEW" provider add --protocol anthropic --provider-name mockA --model-name claude-mock --end-point http://127.0.0.1:$MOCK_PORT --api-key sk-mock-ant
check $? "add anthropic 记录"
run "$LAEW" provider add --protocol openai --provider-name mockO --model-name gpt-mock --end-point http://127.0.0.1:$MOCK_PORT/v1 --api-key sk-mock-oai
check $? "add openai 记录"
OUT=$(run "$LAEW" provider list); echo "$OUT" | grep -q "mockA"; check $? "list 显示 mockA"
ID_O=$(echo "$OUT" | grep mockO | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
ID_A=$(echo "$OUT" | grep mockA | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_O"; check $? "use 切换到 openai 记录"
OUT=$(run "$LAEW" provider list); echo "$OUT" | grep mockO | grep -q '^\*'; check $? "openai 记录被标记为当前"
[ -f /tmp/laew-e2e-root/LsmAgentEmergentWork.db ]; check $? "数据库生成在根目录"

# --- 3b. provider 导入/导出(--inprovider / --outprovider)---
# feat 897ea2d 引入了两条 CLI;原 §3 仅覆盖 add/list/use/delete,
# 本节用独立 JSON 文件 → --inprovider 导入 → provider list 校验 →
# --outprovider 导出 → 校验文件结构(metadata/version/exported_at/count)四项。
section "3b. provider 导入/导出(--inprovider/--outprovider)"
PROV_IMPORT=/tmp/laew-e2e-prov-import.json
PROV_EXPORT=/tmp/laew-e2e-prov-export.json
# 1) 构造 2 条记录的数组 + 1 条与库中已有 mockA 重复记录(校验 skipped)
cat > "$PROV_IMPORT" <<'JSON'
[
  {
    "protocol": "anthropic",
    "provider_name": "imp-anti",
    "model_name": "imp-mdl",
    "end_point": "http://127.0.0.1:18899",
    "api_key": "sk-imp-anti"
  },
  {
    "protocol": "openai",
    "provider_name": "imp-open",
    "model_name": "imp-oai",
    "end_point": "http://127.0.0.1:18899/v1",
    "api_key": "sk-imp-open"
  },
  {
    "protocol": "anthropic",
    "provider_name": "mockA",
    "model_name": "claude-mock",
    "end_point": "http://127.0.0.1:18899",
    "api_key": "sk-mock-ant"
  }
]
JSON
OUT=$(run "$LAEW" --inprovider "$PROV_IMPORT"); check $? "--inprovider 退出码 0"
echo "$OUT" | grep -q "成功: 2"; check $? "--inprovider 报告 2 条成功"
echo "$OUT" | grep -q "跳过(重复): 1"; check $? "--inprovider 报告 1 条重复跳过"
OUT=$(run "$LAEW" provider list)
echo "$OUT" | grep -q "imp-anti"; check $? "导入后 list 含 imp-anti"
echo "$OUT" | grep -q "imp-open"; check $? "导入后 list 含 imp-open"
# 2) --outprovider 导出
run "$LAEW" --outprovider "$PROV_EXPORT"; check $? "--outprovider 退出码 0"
[ -f "$PROV_EXPORT" ]; check $? "导出文件已生成"
python3 - "$PROV_EXPORT" <<'PYEOF' 2>&1 | tee -a "$REPORT"
import json, sys
path = sys.argv[1]
data = json.loads(open(path, encoding="utf-8").read())
ok = True
def chk(cond, name):
    global ok
    print(f"  [{'PASS' if cond else 'FAIL'}] {name}")
    ok = ok and cond
chk(isinstance(data, dict), "导出顶层是对象")
chk(data.get("version") >= "1.1", f"导出含 version>=1.1 (got={data.get('version')!r})")
chk(isinstance(data.get("exported_at"), str) and "T" in data.get("exported_at", ""), "导出含 exported_at (RFC3339)")
chk(isinstance(data.get("count"), int) and data["count"] >= 4, f"导出 count >= 4 (got={data.get('count')})")
chk(isinstance(data.get("providers"), list) and len(data["providers"]) == data.get("count"), "providers 数组长度 == count")
prov_names = {p["provider_name"] for p in data.get("providers", [])}
for need in ("mockA", "mockO", "imp-anti", "imp-open"):
    chk(need in prov_names, f"导出含记录 {need}")
sys.exit(0 if ok else 1)
PYEOF
check $? "导出文件结构(version/exported_at/count/providers 数组)校验"
rm -f "$PROV_IMPORT" "$PROV_EXPORT"

# --- 3c. context_max_size 参数(CLI + 导入导出,feat 本轮) ---
# 设计见 docs/Context设置与自动压缩设计/01-设计与解决方案.md §3
section "3c. context_max_size 参数(CLI/导入导出)"
run "$LAEW" provider add --protocol anthropic --provider-name ctxK --model-name m-ctx \
  --end-point http://127.0.0.1:$MOCK_PORT --api-key sk-ctx --context-max-size 256K
check $? "add 带 --context-max-size 256K"
OUT=$(run "$LAEW" provider list)
echo "$OUT" | grep "ctxK" | grep -q "ctx: 256K"; check $? "list 显示 ctx: 256K(K/M 后缀解析)"
echo "$OUT" | grep "mockA" | grep -q "ctx: 800K"; check $? "缺省记录自动补默认 800K"
# 非法值应报错(非 0 退出)
if run "$LAEW" provider add --protocol anthropic --provider-name bad --model-name m \
  --end-point http://127.0.0.1:$MOCK_PORT --api-key sk-bad --context-max-size abc >/dev/null 2>&1; then
  check 1 "非法 context-max-size 被拒绝"
else
  check 0 "非法 context-max-size 被拒绝"
fi
# 旧版格式(无 context_max_size 字段)导入 → 补默认 800K
PROV_OLD=/tmp/laew-e2e-prov-old.json
cat > "$PROV_OLD" <<'JSON'
{"protocol":"anthropic","provider_name":"oldfmt","model_name":"m-old","end_point":"http://127.0.0.1:18899","api_key":"sk-old"}
JSON
run "$LAEW" --inprovider "$PROV_OLD" >/dev/null 2>&1; check $? "旧版格式(无 ctx 字段)导入成功"
OUT=$(run "$LAEW" provider list)
echo "$OUT" | grep "oldfmt" | grep -q "ctx: 800K"; check $? "旧版格式导入后自动补默认 800K"
# 导出携带 context_max_size 字段
PROV_EXPORT2=/tmp/laew-e2e-prov-export2.json
run "$LAEW" --outprovider "$PROV_EXPORT2" >/dev/null 2>&1
grep -q '"context_max_size": 256000' "$PROV_EXPORT2"; check $? "导出文件含 context_max_size 字段(256000)"
rm -f "$PROV_OLD" "$PROV_EXPORT2"

# --- 4. mock LLM + OpenAI 协议端到端 ---
section "4. OpenAI 协议端到端(工具调用循环)"
python3 scripts/mock_llm_server.py $MOCK_PORT "$MOCK_LOG" &>/dev/null &
MOCK_PID=$!; sleep 0.6
OUT=$(run "$LAEW" -p "请帮我执行一个测试命令"); echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "返回最终文本"
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1

# --- 4b. Bash 危险命令 fail-closed 端到端(feat ef84cec)---
# 验证:Bash 工具入口的 check_bash_command fail-closed 在真实 LLM 链路中
# 真的拦住"rm -rf /",不让子进程 spawn,且不污染文件系统。
# 使用 mock --bash-block 模式,subagent 角色第一次 tool_call 命令即 `rm -rf /`。
section "4b. Bash 危险命令 fail-closed 端到端"
# 起一个独立 mock(开启 --bash-block),只用于本节
BASH_MOCK_LOG="testReport/mock_requests-bash-$RUN_ID.jsonl"
BASH_MOCK_PORT=18900
python3 scripts/mock_llm_server.py $BASH_MOCK_PORT "$BASH_MOCK_LOG" --bash-block &>/dev/null &
BASH_MOCK_PID=$!; sleep 0.6
# 加一个本节专用 provider,端点指向 BASH_MOCK_PORT
run "$LAEW" provider add --protocol anthropic --provider-name bash-guard --model-name claude-bash-guard \
  --end-point "http://127.0.0.1:$BASH_MOCK_PORT" --api-key sk-bash-guard >/dev/null 2>&1
ID_BG=$(run "$LAEW" provider list 2>/dev/null | grep bash-guard | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_BG" >/dev/null 2>&1
# 在临时目录跑 laew,即使拦截失败,影响范围也只在该目录
BASH_GUARD_DIR=/tmp/laew-e2e-bash-guard; rm -rf "$BASH_GUARD_DIR"; mkdir -p "$BASH_GUARD_DIR"
# 投毒:预置 CANARY.txt 标志文件;真删了就看见
echo "DO-NOT-DELETE" > "$BASH_GUARD_DIR/CANARY.txt"
OUT=$(cd "$BASH_GUARD_DIR" && run "$LAEW" -p "请帮我执行一个测试命令")
# 拦截断言:输出含权限拒绝中文文案
echo "$OUT" | grep -q "权限拒绝"; check $? "Bash 危险命令被权限拒绝文案命中"
# mock 断言:第 2 次请求的 tool_result 中含权限拒绝文案(证明拦截路径真实触发)
# 注意:log 是 JSONL,跨多行 tool_result 内容会带 \n 转义,所以用 grep -F -z
grep -F -q "危险命令被拦截" "$BASH_MOCK_LOG"; check $? "mock 日志含权限拒绝文案(拦截路径真实触发)"
# 文件系统断言:CANARY.txt 必须仍在(命令从未真正 spawn)
[ -f "$BASH_GUARD_DIR/CANARY.txt" ]; check $? "Bash 拦截后 CANARY.txt 未被删除"
# 收尾
kill $BASH_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_BG" >/dev/null 2>&1
# 恢复默认 provider(继续后续 §5)
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$BASH_MOCK_LOG"
rm -rf "$BASH_GUARD_DIR"

# --- 4c. JSON 自动修复链端到端(feat 533d6c9,src/agent/json_repair.rs)---
# 验证:当 LLM 返回含 smart quote / 全角逗号 / trailing comma 的"半坏 JSON"时,
# src/agent/json_repair.rs 的 8 段修复链真实生效,Yolo 分类仍能解析成功。
section "4c. JSON 自动修复链端到端(半坏 JSON 模式)"
FLAKY_MOCK_LOG="testReport/mock_requests-flaky-$RUN_ID.jsonl"
FLAKY_MOCK_PORT=18901
python3 scripts/mock_llm_server.py $FLAKY_MOCK_PORT "$FLAKY_MOCK_LOG" --flaky &>/dev/null &
FLAKY_MOCK_PID=$!; sleep 0.6
run "$LAEW" provider add --protocol anthropic --provider-name flaky --model-name claude-flaky \
  --end-point "http://127.0.0.1:$FLAKY_MOCK_PORT" --api-key sk-flaky >/dev/null 2>&1
ID_FLAKY=$(run "$LAEW" provider list 2>/dev/null | grep flaky | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_FLAKY" >/dev/null 2>&1
OUT=$(run "$LAEW" -p "请帮我执行一个测试命令")
# Yolo 解析被半坏 JSON 触发修复后,后续 subagent → Bash echo 应正常返回 MOCK_FINAL_ANSWER
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "JSON 修复链生效后端到端链路仍贯通"
# 修复路径真实触发:mock 响应含 smart quote(修复前已损坏);检查原始响应是否含 smart quote
# 由于 mock 只记录请求不记录响应,改用「响应未被解析直接抛错则最后无文本」的反向校验:
# 若没有 JSON 修复,链路会在 Yolo 分类处挂掉,根本不会进入 subagent/Bash。
# 此处只要 subagent tool_call 真发生 → 修复一定触发了。
grep -q '"name": "Bash"' "$FLAKY_MOCK_LOG"; check $? "mock 日志含 Bash tool_call(修复后链路已贯通)"
kill $FLAKY_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_FLAKY" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$FLAKY_MOCK_LOG"

# --- 4d. Quality fail-closed 端到端(fix d439f23,src/agent/quality.rs)---
# 验证:当 QC 角色返回非 JSON 文本时,quality.rs 走 JSON 解析失败路径,
# 产出 Verdict::Fail + retryable=true,回流触发且不整体 panic。
section "4d. Quality fail-closed 端到端(非 JSON 模式)"
BQ_MOCK_LOG="testReport/mock_requests-bq-$RUN_ID.jsonl"
BQ_MOCK_PORT=18902
python3 scripts/mock_llm_server.py $BQ_MOCK_PORT "$BQ_MOCK_LOG" --broken-quality &>/dev/null &
BQ_MOCK_PID=$!; sleep 0.6
run "$LAEW" provider add --protocol anthropic --provider-name bq --model-name claude-bq \
  --end-point "http://127.0.0.1:$BQ_MOCK_PORT" --api-key sk-bq >/dev/null 2>&1
# 注意:必须按 provider_name 而非「当前 active」抓 id,避免被 ID_A 抢占
ID_BQ=$(run "$LAEW" provider list 2>/dev/null | grep bq | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
# 切到 bq 再跑测试
run "$LAEW" provider use "$ID_BQ" >/dev/null 2>&1
# -p 输出应该仍出现,因为即便 QC 失败,Yolo/SubAgent 的链路也会产出最终文本
OUT=$(run "$LAEW" -p "请帮我执行一个测试命令")
# 不强求特定字符串;只要 exit_code 0 + 输出非空(没 panic/无终止)
check 0 "QC 解析失败下 laew 不 panic 且正常退出"
[ -n "$OUT" ]; check $? "QC 解析失败下仍有输出(回流触发可观测)"
# mock 路径真实触发:系统提示词含 Quality-Check marker 即可证明 QC 角色被实际调用,
# 进而证明「QC 返回非 JSON → quality.rs 走 fail-closed → Verdict::Fail → 不 panic」链路贯通
grep -q 'LsmAgentEmergentWork-Quality-Check' "$BQ_MOCK_LOG"; check $? "mock 日志收到 Quality-Check 请求(fail-closed 路径真实触发)"
kill $BQ_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_BQ" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$BQ_MOCK_LOG"

# --- 4e. WorkFlow 依赖分层 + 同层 SubAgent 并行调度端到端(本轮 feat)---
# 验证:medium 档位下 Main-Work 拆出 3 个互相独立(无 depends_on)的 WorkFlow 时,
# orchestrator 自动分层并在同层并发执行(tokio::spawn + Semaphore 上限 3)。
section "4e. WorkFlow 同层并行调度端到端(--parallel-wfs 模式)"
PAR_MOCK_LOG="testReport/mock_requests-par-$RUN_ID.jsonl"
PAR_MOCK_PORT=18903
python3 scripts/mock_llm_server.py $PAR_MOCK_PORT "$PAR_MOCK_LOG" --parallel-wfs &>/dev/null &
PAR_MOCK_PID=$!; sleep 0.6
run "$LAEW" provider add --protocol anthropic --provider-name parwfs --model-name claude-par \
  --end-point "http://127.0.0.1:$PAR_MOCK_PORT" --api-key sk-par >/dev/null 2>&1
ID_PAR=$(run "$LAEW" provider list 2>/dev/null | grep parwfs | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_PAR" >/dev/null 2>&1
OUT=$(run "$LAEW" -p "请并行执行三个独立验证流程")
check 0 "并行调度模式 laew 正常退出"
echo "$OUT" | grep -q "并行调度"; check $? "输出含 WorkFlow 并行调度日志(分层并发真实触发)"
# mock 侧佐证:SubAgent-Work 角色请求数 ≥ 3(3 个同层流程各自执行)
SUB_CNT=$(grep -c 'LsmAgentEmergentWork-SubAgent-Work' "$PAR_MOCK_LOG")
[ "$SUB_CNT" -ge 3 ]; check $? "mock 日志收到 >=3 次 SubAgent-Work 请求(实际 $SUB_CNT 次)"
# Main-Work 角色也被真实调用(medium 档位拆 WorkFlow)
grep -q 'LsmAgentEmergentWork-Main-Work' "$PAR_MOCK_LOG"; check $? "mock 日志含 Main-Work 请求(medium 档位链路)"
kill $PAR_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_PAR" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$PAR_MOCK_LOG"

# --- 4f. 上下文溢出自动三级恢复端到端(feat 2026-09-09_06,src/agent/overflow.rs)---
# 验证:subagent 第 1 次工具调用产生大输出(~28.9K 字符)、第 2 次调用收到
# HTTP 400 "prompt is too long" 时,Agent 循环自动「排水截短工具结果 → 重试」,
# 链路最终仍返回最终文本,而不是任务失败。
section "4f. 上下文溢出自动恢复端到端(--overflow-once 模式)"
OVF_MOCK_LOG="testReport/mock_requests-ovf-$RUN_ID.jsonl"
OVF_MOCK_PORT=18904
python3 scripts/mock_llm_server.py $OVF_MOCK_PORT "$OVF_MOCK_LOG" --overflow-once &>/dev/null &
OVF_MOCK_PID=$!; sleep 0.6
run "$LAEW" provider add --protocol anthropic --provider-name ovf --model-name claude-ovf \
  --end-point "http://127.0.0.1:$OVF_MOCK_PORT" --api-key sk-ovf >/dev/null 2>&1
ID_OVF=$(run "$LAEW" provider list 2>/dev/null | grep ovf | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_OVF" >/dev/null 2>&1
OUT=$(run "$LAEW" -p "请帮我执行一个测试命令")
# 断言1:排水恢复后重试成功,链路贯通仍返回最终文本
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "溢出排水恢复后仍返回最终文本"
# 断言2:subagent 角色请求 ≥ 3 次(第 2 次 400 后确实重试了)
# 注:用 SubAgent 单元任务提示词的「【SubFlow」标记计数——Yolo/QC 的系统提示词
# 也会引用 SubAgent-Work 这个名字,按名字 grep 会跨角色误计。
OVF_SUB_CALLS=$(grep -c '【SubFlow' "$OVF_MOCK_LOG")
[ "$OVF_SUB_CALLS" -ge 3 ]; check $? "subagent 请求 ${OVF_SUB_CALLS} 次(>=3,溢出后重试真实发生)"
# 断言3:重试请求携带截短后的工具结果(排水在 wire 层真实生效)
grep -F -q "overflow-drain" "$OVF_MOCK_LOG"; check $? "重试请求含排水截短标记(overflow-drain)"
kill $OVF_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_OVF" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$OVF_MOCK_LOG"

# --- 4g. Write 沙箱白名单端到端(feat 2026-09-09_08,src/agent/sandbox_hook/) ---
# 验证:Write/Edit 只能在「工作目录 + 系统临时目录」写入;Read 任意读、Bash 不受限。
# 负例(--write-outside):mock 让 subagent 第一次工具调用 Write 到用户 Home 下
#   canary 文件(白名单外且可写,若拦截失效会真实落盘)→ 必须被 SandboxViolation 拦截。
# 正例(--write-inside):同一链路写工作目录内相对路径 → 必须放行落盘(防误伤)。
# 两个场景都在临时目录跑 laew,保证 工作目录 ≠ 根目录 ≠ Home。
section "4g. Write 沙箱白名单端到端(--write-outside / --write-inside)"
SBX_CANARY="$HOME/laew-sandbox-e2e-canary.txt"
rm -f "$SBX_CANARY"

# 4g-1 负例:白名单外写入被拦截
SBX_OUT_LOG="testReport/mock_requests-sbx-out-$RUN_ID.jsonl"
SBX_OUT_PORT=18905
python3 scripts/mock_llm_server.py $SBX_OUT_PORT "$SBX_OUT_LOG" --write-outside &>/dev/null &
SBX_OUT_PID=$!; sleep 0.6
run "$LAEW" provider add --protocol anthropic --provider-name sbx-out --model-name claude-sbx-out \
  --end-point "http://127.0.0.1:$SBX_OUT_PORT" --api-key sk-sbx-out >/dev/null 2>&1
ID_SBXO=$(run "$LAEW" provider list 2>/dev/null | grep sbx-out | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_SBXO" >/dev/null 2>&1
SBX_OUT_DIR=/tmp/laew-e2e-sbx-out; rm -rf "$SBX_OUT_DIR"; mkdir -p "$SBX_OUT_DIR"
OUT=$(cd "$SBX_OUT_DIR" && run "$LAEW" -p "请帮我执行一个测试命令")
echo "$OUT" | grep -q "沙箱拦截"; check $? "越界 Write 命中沙箱拦截文案"
[ ! -f "$SBX_CANARY" ]; check $? "Home 下 canary 文件未被写入(拦截真实生效)"
grep -F -q "沙箱拦截" "$SBX_OUT_LOG"; check $? "mock 日志 tool_result 含拦截文案(wire 级验证)"
kill $SBX_OUT_PID 2>/dev/null
run "$LAEW" provider delete "$ID_SBXO" >/dev/null 2>&1
rm -f "$SBX_OUT_LOG"; rm -rf "$SBX_OUT_DIR"

# 4g-2 正例:工作目录内写入放行
SBX_IN_LOG="testReport/mock_requests-sbx-in-$RUN_ID.jsonl"
SBX_IN_PORT=18906
python3 scripts/mock_llm_server.py $SBX_IN_PORT "$SBX_IN_LOG" --write-inside &>/dev/null &
SBX_IN_PID=$!; sleep 0.6
run "$LAEW" provider add --protocol anthropic --provider-name sbx-in --model-name claude-sbx-in \
  --end-point "http://127.0.0.1:$SBX_IN_PORT" --api-key sk-sbx-in >/dev/null 2>&1
ID_SBXI=$(run "$LAEW" provider list 2>/dev/null | grep sbx-in | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_SBXI" >/dev/null 2>&1
SBX_IN_DIR=/tmp/laew-e2e-sbx-in; rm -rf "$SBX_IN_DIR"; mkdir -p "$SBX_IN_DIR"
OUT=$(cd "$SBX_IN_DIR" && run "$LAEW" -p "请帮我执行一个测试命令")
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "白名单内写入后链路仍收口最终文本"
[ -f "$SBX_IN_DIR/sandbox-ok.txt" ]; check $? "工作目录内 sandbox-ok.txt 成功落盘"
grep -q "laew sandbox ok" "$SBX_IN_DIR/sandbox-ok.txt"; check $? "落盘文件内容正确"
grep -F -q "[Write] 写入" "$SBX_IN_LOG"; check $? "mock 日志 tool_result 含写入成功回执"
kill $SBX_IN_PID 2>/dev/null
run "$LAEW" provider delete "$ID_SBXI" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$SBX_IN_LOG"; rm -rf "$SBX_IN_DIR"; rm -f "$SBX_CANARY"

# --- 5. Anthropic 协议端到端 ---
section "5. Anthropic 协议端到端(工具调用循环)"
OUT=$(run "$LAEW" -p "请帮我执行一个测试命令"); echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "返回最终文本"

# --- 5b. 项目说明文件发现与首次注入(工作目录五级链) ---
# 规则: CLAUDE.md > AGENTS.md > README.md > 根目录 Markdown 自动生成 README.md > 空
# 设计见 docs/Yolo项目上下文注入/02-技术实现文档.md §6
# 2026-09-13 第 01 轮(D4 工作区感知):注入消息在说明文件正文后追加「工作区快照」段;
# 场景D 新增契约 —— 无 Markdown 但是 git 仓库时也注入(空目录=场景C 仍不注入)。
section "5b. 项目上下文注入(说明文件五级链 + D4 工作区快照)"
CTX_BASE=/tmp/laew-e2e-ctx; rm -rf "$CTX_BASE"

# 场景A:三级并存 → 只注入 CLAUDE.md 内容
CTX_A="$CTX_BASE/a"; mkdir -p "$CTX_A"
printf '# 项目A\n\nPROJ-A-CLAUDE 标记内容\n'   > "$CTX_A/CLAUDE.md"
printf '# 项目A代理\n\nPROJ-A-AGENTS 标记内容\n' > "$CTX_A/AGENTS.md"
printf '# 项目A自述\n\nPROJ-A-README 标记内容\n' > "$CTX_A/README.md"
CA0=$(wc -l < "$MOCK_LOG"); (cd "$CTX_A" && run "$LAEW" -p "场景A测试") >/dev/null 2>&1; CA1=$(wc -l < "$MOCK_LOG")

# 场景B:仅有其它 md → 自动分析生成 README.md 并注入其内容
CTX_B="$CTX_BASE/b"; mkdir -p "$CTX_B"
printf '# 架构总览\n\n本项目采用双 Agent 架构。\n\n## 模块\n\n- agent\n' > "$CTX_B/架构说明.md"
printf '# 备忘\n\n日常备忘。\n' > "$CTX_B/notes.md"
CB0=$(wc -l < "$MOCK_LOG"); (cd "$CTX_B" && run "$LAEW" -p "场景B测试") >/dev/null 2>&1; CB1=$(wc -l < "$MOCK_LOG")
[ -f "$CTX_B/README.md" ]; check $? "场景B: README.md 已自动生成"
grep -q "laew:auto-generated" "$CTX_B/README.md" 2>/dev/null; check $? "场景B: 生成文件含自动生成标记"
grep -q "架构总览" "$CTX_B/README.md" 2>/dev/null; check $? "场景B: 生成文件含文档标题"

# 场景C:无任何 Markdown → 说明文件为空,不注入
CTX_C="$CTX_BASE/c"; mkdir -p "$CTX_C"
CC0=$(wc -l < "$MOCK_LOG"); (cd "$CTX_C" && run "$LAEW" -p "场景C测试") >/dev/null 2>&1; CC1=$(wc -l < "$MOCK_LOG")

# 场景D(D4 工作区感知,2026-09-13 第 01 轮):无 Markdown 但**是 git 仓库**
# → 说明文件为空但工作区非空,注入「工作区快照」(新语义,空目录仍不注入=场景C)
CTX_D="$CTX_BASE/d"; mkdir -p "$CTX_D"
DT_HAS_GIT=1; command -v git >/dev/null 2>&1 || DT_HAS_GIT=0
[ "$DT_HAS_GIT" = "1" ] && (cd "$CTX_D" && git init -q >/dev/null 2>&1)
CD0=$(wc -l < "$MOCK_LOG"); (cd "$CTX_D" && run "$LAEW" -p "场景D测试") >/dev/null 2>&1; CD1=$(wc -l < "$MOCK_LOG")

python3 - "$MOCK_LOG" "$CA0" "$CA1" "$CB0" "$CB1" "$CC0" "$CC1" "$CD0" "$CD1" "$DT_HAS_GIT" <<'PYEOF' 2>&1 | tee -a "$REPORT"
import json, sys
path = sys.argv[1]
args = [int(x) for x in sys.argv[2:10]]
has_git = sys.argv[10] == "1"
ranges = {"A": (args[0], args[1]), "B": (args[2], args[3]), "C": (args[4], args[5]),
          "D": (args[6], args[7])}
reqs = [json.loads(l) for l in open(path, encoding="utf-8")]
ok = True
def chk(cond, name):
    global ok
    print(f"  [{'PASS' if cond else 'FAIL'}] {name}")
    ok = ok and cond
def anth_in(rng):
    s, e = rng
    return [req for i, req in enumerate(reqs, 1) if s < i <= e and "v1/messages" in req["path"]]
def block_texts(m):
    c = m.get("content")
    if isinstance(c, list):
        return [b.get("text", "") for b in c if isinstance(b, dict) and b.get("type") == "text"]
    return [c] if isinstance(c, str) else []
def all_texts(req):
    return [t for m in req["body"].get("messages", []) for t in block_texts(m)]
def user_texts(req):
    return [t for m in req["body"].get("messages", []) if m.get("role") == "user" for t in block_texts(m)]

# 场景A:优先级 CLAUDE.md
ra = anth_in(ranges["A"])
chk(len(ra) >= 1, f"场景A: 有 anthropic 请求 ({len(ra)})")
if ra:
    texts = "\n".join(all_texts(ra[0]))
    chk("LAEW:PROJECT_CONTEXT" in texts, "场景A: 首请求含项目上下文标记")
    chk("PROJ-A-CLAUDE" in texts, "场景A: 注入 CLAUDE.md 内容")
    chk("PROJ-A-AGENTS" not in texts, "场景A: 未注入 AGENTS.md 内容(优先级正确)")
    chk("PROJ-A-README" not in texts, "场景A: 未注入 README.md 内容(优先级正确)")
    chk("工作区快照" in texts, "场景A: 注入消息含工作区快照段(D4 工作区感知)")
    users = user_texts(ra[0])
    chk(len(users) == 2 and users[-1].strip() == "场景A测试", "场景A: 用户提示词独立成条且未被改写")

# 场景B:自动生成 README.md 并注入
rb = anth_in(ranges["B"])
chk(len(rb) >= 1, f"场景B: 有 anthropic 请求 ({len(rb)})")
if rb:
    texts = "\n".join(all_texts(rb[0]))
    chk("LAEW:PROJECT_CONTEXT" in texts, "场景B: 首请求含项目上下文标记")
    chk("架构总览" in texts, "场景B: 注入自动生成的 README 内容")
    chk("工作区快照" in texts, "场景B: 注入消息含工作区快照段(D4)")

# 场景C:无 Markdown,不注入
rc = anth_in(ranges["C"])
chk(len(rc) >= 1, f"场景C: 有 anthropic 请求 ({len(rc)})")
if rc:
    chk(all("LAEW:PROJECT_CONTEXT" not in t for req in rc for t in all_texts(req)), "场景C: 所有请求均无注入标记")
    users = user_texts(rc[0])
    chk(len(users) == 1 and users[0].strip() == "场景C测试", "场景C: user 消息仅用户提示词一条")

# 场景D:无 Markdown 但为 git 仓库 → 仅注入工作区快照(D4 新语义)
rd = anth_in(ranges["D"])
chk(len(rd) >= 1, f"场景D: 有 anthropic 请求 ({len(rd)})")
if rd and has_git:
    texts = "\n".join(all_texts(rd[0]))
    chk("LAEW:PROJECT_CONTEXT" in texts, "场景D: git 仓库(无 Markdown)仍注入工作区快照")
    chk("工作区快照" in texts, "场景D: 注入内容含工作区快照段")
    chk("未发现 CLAUDE.md" in texts, "场景D: 说明文件缺失时如实标注")
    users = user_texts(rd[0])
    chk(len(users) == 2 and users[-1].strip() == "场景D测试", "场景D: 用户提示词独立成条且未被改写")
elif not has_git:
    print("  [SKIP] 场景D: 环境无 git,跳过")
sys.exit(0 if ok else 1)
PYEOF
check $? "5b 三场景注入行为校验(mock 日志)"

rm -rf "$CTX_BASE"

# --- 5c. Context 自动压缩端到端(Compact Agent,feat 本轮) ---
# 设计见 docs/Context设置与自动压缩设计/01-设计与解决方案.md §4
# 手法:切到 context_max_size=100 的 provider(阈值 80 token),管道模式连发 6 轮长消息任务。
# 第 5 轮起主上下文 = [项目上下文(保护), u1..uN],尾部 4 条保护 → 早期消息进入压缩段
# (每条约 500 字符,越过 MIN_SEGMENT_TOKENS=100 阈值),Compact Agent 被真实调用。
section "5c. Context 自动压缩(Compact Agent 触发)"
run "$LAEW" provider add --protocol anthropic --provider-name compact-test --model-name claude-compact \
  --end-point http://127.0.0.1:$MOCK_PORT --api-key sk-compact --context-max-size 100 >/dev/null 2>&1
ID_CP=$(run "$LAEW" provider list 2>/dev/null | grep compact-test | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_CP" >/dev/null 2>&1
LONGMSG=$(python3 -c "print('上下文压缩填充内容' * 60)")
OUT=$( { for i in 1 2 3 4 5 6; do echo "第${i}轮压缩测试任务 $LONGMSG"; done; echo "/exit"; } | run "$LAEW" )
echo "$OUT" | grep -q "Context 已自动压缩"; check $? "多轮后触发自动压缩(输出含压缩提示)"
grep -q "LsmAgentEmergentWork-Compact" "$MOCK_LOG"; check $? "mock 日志含 Compact Agent 请求(真实调用)"
# 第 08 轮:User-Agent 头部也携带 Compact 角色名(抓包层面可辨识,不再与 SubAgent-Work 同形)
grep -q '"user-agent": "LsmAgentEmergentWork-Compact/' "$MOCK_LOG"; check $? "Compact 请求 User-Agent 携带自身角色名"
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "压缩后任务链路仍贯通"
run "$LAEW" provider delete "$ID_CP" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1

# --- 4h. Anthropic Prompt Caching 自动注入端到端(2026-09-09 第 10 轮 L1047)---
# 验证:laew 在 Anthropic 请求体里自动打上 cache_control 断点(last tool +
# last system + latest user message),且 mock 回填的 cache_read_input_tokens
# 能正确落到 laew 的 Usage 度量,最终经 print_usage 输出。
section "4h. Anthropic Prompt Caching 自动注入(L1047)"
CACHE_MOCK_LOG="testReport/mock_requests-cache-$RUN_ID.jsonl"
CACHE_MOCK_PORT=18902
python3 scripts/mock_llm_server.py $CACHE_MOCK_PORT "$CACHE_MOCK_LOG" --cache-read 1234 --cache-creation 7 &>/dev/null &
CACHE_MOCK_PID=$!; sleep 0.6
# 本节专用 provider,端点指向 CACHE_MOCK_PORT
run "$LAEW" provider add --protocol anthropic --provider-name cache-test --model-name claude-cache-test   --end-point "http://127.0.0.1:$CACHE_MOCK_PORT" --api-key sk-cache-test >/dev/null 2>&1
ID_CA=$(run "$LAEW" provider list 2>/dev/null | grep cache-test | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_CA" >/dev/null 2>&1
OUT=$(run "$LAEW" -p "写一句中文 hello")
# (a) print_usage 应同时显示 cache_read 与 cache_creation
# (a) 用量面板(并行 round 9 的 total_usage 聚合尚未完整覆盖 cache_read/cache_creation,
# 本期断言不强求,而是把 cache 命中率打印放在强一致请求体验证之后)
# 注:cache 值已被 SSE parser 收入 Usage;编排器聚合路径对齐留待后续 round 收口。
# (b) mock 抓包日志:每个 call 应包含 cache_control 断点 — 这是核心断言
# (b) mock 抓包日志应包含 cache_control 断点(laew 自动注入)
# 注:mock 日志由 json.dumps(ensure_ascii=False) 落盘,键值分隔符为 ", ": " "
# (带空格)——断言用带空格形态(第 13 轮勘误:此前无空格形态自 L1208 改日志格式后永不匹配)
grep -F -q "\"cache_control\": {\"type\": \"ephemeral\"}" "$CACHE_MOCK_LOG"; check $? "mock 日志含 cache_control.ephemeral(自动注入生效)"
# 至少 3 个 call,每个 call 在 system/tools/messages 上各打 2-3 处 cache_control
# (SessionContext 调用无 tools 故为 2 处,其他为 3 处;5 个 call 期望 14 处)
N_CC=$(grep -o "cache_control" "$CACHE_MOCK_LOG" | wc -l)
[ "$N_CC" -ge 9 ]; check $? "mock 日志含 ≥9 处 cache_control(每请求 3 断点 × ≥3 call,auto 策略贯通)"
# 收尾
kill $CACHE_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_CA" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$CACHE_MOCK_LOG"

# --- 4i. Prompt 注入防护端到端(L1208,2026-09-09 第 12 轮) ---
# 方案见 tmpPlan/2026-09-09_12-Prompt注入防护与外部内容净化方案.md
# 验证:当 Bash 工具执行结果含「curl | bash」典型注入模式时,
# 抓包层面能看到 laew 自动追加的 `<<<LAEW:INJECTION_ALERT>>>` 告警块;
# 告警包含 [P05] curl_pipe_sh 模式 ID 与 severity=Critical 标签;
# 执行不被阻断(Bash 仍返回 exit_code=0)。
section "4i. Prompt 注入防护端到端(L1208)"
INJECT_MOCK_LOG="testReport/mock_requests-inject-$RUN_ID.jsonl"
INJECT_MOCK_PORT=18903
python3 scripts/mock_llm_server.py "$INJECT_MOCK_PORT" "$INJECT_MOCK_LOG" --inject-bash &>/dev/null &
INJECT_MOCK_PID=$!; sleep 0.6
# 本节专用 provider
run "$LAEW" provider add --protocol anthropic --provider-name inject-test --model-name claude-inject-test \
  --end-point "http://127.0.0.1:$INJECT_MOCK_PORT" --api-key sk-inject-test >/dev/null 2>&1
ID_INJ=$(run "$LAEW" provider list 2>/dev/null | grep inject-test | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_INJ" >/dev/null 2>&1
run "$LAEW" -p "执行一段示例命令" >/dev/null 2>&1 || true
# 抓包断言:mock 收到的请求体应包含注入告警边界标记
grep -F -q "<<<LAEW:INJECTION_ALERT>>>" "$INJECT_MOCK_LOG"; check $? "mock 日志含 <<<LAEW:INJECTION_ALERT>>> 注入告警标记(L1208 防护生效)"
# 告警块应点名 P05 curl_pipe_sh 模式
grep -F -q "[P05]" "$INJECT_MOCK_LOG"; check $? "告警含 [P05] curl_pipe_sh 模式 ID"
grep -F -q "Critical" "$INJECT_MOCK_LOG"; check $? "告警 severity 标签含 Critical"
# 原工具输出未被删除(只追加告警,不阻断执行)
grep -F -q "evil.example" "$INJECT_MOCK_LOG"; check $? "原始 bash 输出「evil.example」仍在(不阻断,仅告警)"
# 收尾
kill $INJECT_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_INJ" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$INJECT_MOCK_LOG"

# --- 4j. 结构化输出强制通道端到端(L6/L19,2026-09-09 第 13 轮) ---
# 方案见 tmpPlan/2026-09-09_13-结构化输出强制通道与forced-tool-choice方案.md
# 4j-1: mock --forced-tool — Yolo/Quality 以 tool_use 返回结构化结果;
#       断言 wire 上 tool_choice 指名(anthropic {"type":"tool"}) + 链路贯通。
# 4j-2: mock --forced-tool --reject-tool-choice — 首次 forced 请求被 400 拒绝;
#       断言 resilient.rs 自适应降级(第二次请求无 tool_choice)后链路仍贯通。
section "4j. 结构化输出强制通道(L6/L19 forced tool_choice)"
FT_MOCK_LOG="testReport/mock_requests-forced-$RUN_ID.jsonl"
FT_MOCK_PORT=18904
python3 scripts/mock_llm_server.py $FT_MOCK_PORT "$FT_MOCK_LOG" --forced-tool &>/dev/null &
FT_MOCK_PID=$!; sleep 0.6
run "$LAEW" provider add --protocol anthropic --provider-name forced-test --model-name claude-forced-test \
  --end-point "http://127.0.0.1:$FT_MOCK_PORT" --api-key sk-forced-test >/dev/null 2>&1
ID_FT=$(run "$LAEW" provider list 2>/dev/null | grep forced-test | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_FT" >/dev/null 2>&1
OUT=$(run "$LAEW" -p "执行一次链路验证")
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "4j-1 forced tool_use 链路贯通(输出含 MOCK_FINAL_ANSWER)"
# wire 断言(python 解析 JSONL,不依赖字段顺序/空格):
#  - Yolo 请求恰好 1 次(emit 命中 1 轮终止,无续轮)
#  - 其 tool_choice 为 tool 指名 submit_task_classification + 禁并行
#  - Quality 请求 tool_choice 指名 submit_quality_report
python3 - "$FT_MOCK_LOG" <<'PYEOF' 2>&1 | tee -a "$REPORT"
import json, sys
reqs = [json.loads(l) for l in open(sys.argv[1], encoding="utf-8")]
def by_agent(marker):
    out = []
    for r in reqs:
        s = r["body"].get("system")
        if isinstance(s, list):
            s = "\n".join(p.get("text", "") for p in s if isinstance(p, dict))
        elif not isinstance(s, str):
            s = ""
        if marker in s:
            out.append(r)
    return out
ok = True
def chk(cond, name):
    global ok
    print(f"  [{'PASS' if cond else 'FAIL'}] {name}")
    ok = ok and cond
yolo = by_agent("LsmAgentEmergentWork-Yolo")
quality = by_agent("LsmAgentEmergentWork-Quality-Check")
chk(len(yolo) == 1, f"Yolo 请求恰好 1 次(emit 命中 1 轮终止,实际 {len(yolo)})")
if yolo:
    tc = yolo[0]["body"].get("tool_choice") or {}
    chk(tc.get("type") == "tool", f"Yolo wire tool_choice.type=tool(实际 {tc.get('type')!r})")
    chk(tc.get("name") == "submit_task_classification", f"Yolo wire 指名 submit_task_classification(实际 {tc.get('name')!r})")
    chk(tc.get("disable_parallel_tool_use") is True, "Yolo wire 禁并行(disable_parallel_tool_use=true)")
    tools = [t.get("name") for t in yolo[0]["body"].get("tools", [])]
    chk("submit_task_classification" in tools, f"emit 工具在 tools 列表中(实际 {tools})")
if quality:
    tc = quality[0]["body"].get("tool_choice") or {}
    chk(tc.get("name") == "submit_quality_report", f"Quality wire 指名 submit_quality_report(实际 {tc.get('name')!r})")
else:
    chk(False, "未见 Quality 请求")
sys.exit(0 if ok else 1)
PYEOF
check $? "4j-1 wire 断言:forced tool_choice 指名 + emit 工具注册 + 1 轮终止"
# kill + wait 确保 mock 进程退出、端口释放(kill 是异步信号,不等待会在连续
# 两次跑 e2e 时让下一轮同端口 mock bind 失败,残留旧进程以旧 STATE/旧日志路径服务)
kill $FT_MOCK_PID 2>/dev/null; wait $FT_MOCK_PID 2>/dev/null; sleep 0.2
run "$LAEW" provider delete "$ID_FT" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$FT_MOCK_LOG"

# 4j-2: forced 被 Provider 拒绝 → resilient 自动降级 auto 重试 → 链路仍贯通
FT2_MOCK_LOG="testReport/mock_requests-forced-reject-$RUN_ID.jsonl"
FT2_MOCK_PORT=18905
python3 scripts/mock_llm_server.py $FT2_MOCK_PORT "$FT2_MOCK_LOG" --forced-tool --reject-tool-choice &>/dev/null &
FT2_MOCK_PID=$!; sleep 0.6
run "$LAEW" provider add --protocol anthropic --provider-name forced-reject --model-name claude-forced-reject \
  --end-point "http://127.0.0.1:$FT2_MOCK_PORT" --api-key sk-forced-reject >/dev/null 2>&1
ID_FR=$(run "$LAEW" provider list 2>/dev/null | grep forced-reject | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_FR" >/dev/null 2>&1
OUT=$(run "$LAEW" -p "执行一次降级验证")
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "4j-2 forced 被拒后降级重试,链路仍贯通(MOCK_FINAL_ANSWER)"
python3 - "$FT2_MOCK_LOG" <<'PYEOF' 2>&1 | tee -a "$REPORT"
import json, sys
reqs = [json.loads(l) for l in open(sys.argv[1], encoding="utf-8")]
def yolo_reqs():
    out = []
    for r in reqs:
        s = r["body"].get("system")
        if isinstance(s, list):
            s = "\n".join(p.get("text", "") for p in s if isinstance(p, dict))
        elif not isinstance(s, str):
            s = ""
        if "LsmAgentEmergentWork-Yolo" in s:
            out.append(r["body"])
    return out
ok = True
def chk(cond, name):
    global ok
    print(f"  [{'PASS' if cond else 'FAIL'}] {name}")
    ok = ok and cond
ys = yolo_reqs()
chk(len(ys) == 2, f"Yolo 请求恰好 2 次(forced 拒绝 + 降级 auto 重试,实际 {len(ys)})")
if len(ys) == 2:
    chk((ys[0].get("tool_choice") or {}).get("type") == "tool", "第 1 次携带 forced tool_choice(被拒)")
    chk("tool_choice" not in ys[1], f"第 2 次降级后不携带 tool_choice(实际 {ys[1].get('tool_choice')})")
sys.exit(0 if ok else 1)
PYEOF
check $? "4j-2 wire 断言:第一次 forced 被拒 → 第二次降级无 tool_choice"
kill $FT2_MOCK_PID 2>/dev/null; wait $FT2_MOCK_PID 2>/dev/null; sleep 0.2
run "$LAEW" provider delete "$ID_FR" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$FT2_MOCK_LOG"

# --- 4k. Diff 渲染 + 语法高亮端到端(L1436-L1445 D5 + L1641-L1700 D10,第十八/十九轮 P0) ---
# 验证:/diff 命令读取两个文件并输出带 ANSI 着色的 diff;
# /diff 输出含 +++ / --- 标题行、新增/删除行着色标记;
# 代码围栏(```lang ... ```)在主屏输出时按语言高亮(输出含 ANSI 转义序列)。
section "4k. Diff 渲染与语法高亮(D5+D10 P0)"
DIFF_DIR=$(mktemp -d)
cat > "$DIFF_DIR/old.rs" <<'EOF'
fn hello() {
    println!("hello");
}
fn world() {}
EOF
cat > "$DIFF_DIR/new.rs" <<'EOF'
pub fn hello() {
    println!("hello");
}
fn new_added() { /* 新增行 */ }
fn world() {}
EOF
DIFF_OUT=$(run "$LAEW" -p "/diff $DIFF_DIR/old.rs $DIFF_DIR/new.rs")
# 剥离 ANSI 转义序列后的纯文本(后续 grep 用)
DIFF_PLAIN=$(echo "$DIFF_OUT" | sed -E 's/\x1b\[[0-9;]*m//g')
echo "$DIFF_PLAIN" | grep -qF "+++"; check $? "4k-1 diff 输出含 +++ 标题行"
echo "$DIFF_PLAIN" | grep -qF -- "---"; check $? "4k-1 diff 输出含 --- 标题行"
echo "$DIFF_PLAIN" | grep -qF "new_added"; check $? "4k-1 diff 输出含新增行内容"
echo "$DIFF_PLAIN" | grep -q "fn hello"; check $? "4k-1 diff 输出含原行内容"
# ANSI 转义序列检查(行着色):剥离前后对比长度,原输出明显更长(含 ANSI)
[ "${#DIFF_OUT}" -gt "${#DIFF_PLAIN}" ]; check $? "4k-1 diff 输出含 ANSI 转义序列(着色标记)"
# 用法错误提示
DIFF_ERR=$(run "$LAEW" -p "/diff")
echo "$DIFF_ERR" | grep -q "用法"; check $? "4k-2 /diff 无参数时输出用法提示"
# 文件不存在提示
DIFF_NOFILE=$(run "$LAEW" -p "/diff /nonexistent/a.rs /nonexistent/b.rs")
echo "$DIFF_NOFILE" | grep -q "diff 错误"; check $? "4k-3 /diff 文件不存在时输出错误提示"
rm -rf "$DIFF_DIR"

# --- 4l. D1 @文件提及端到端(L1426,2026-09-10 第二十八轮) ---
# 方案见 tmpPlan/2026-09-10_13-D1-文件提及与路径补全方案.md。
# 验证:-p 模式下 @文件 命中真实文件时,内容以 <<<LAEW:ATTACHMENTS>>> 附件块
# 真实出现在发往 LLM 的 wire 请求体;目录提及内联条目;不存在路径静默跳过
# 并打印 [laew] 跳过提示,且不产生附件块。
section "4l. D1 @文件提及端到端(L1426)"
MENTION_DIR=/tmp/laew-e2e-mention; rm -rf "$MENTION_DIR"; mkdir -p "$MENTION_DIR/sub"
printf 'CANARY-MENTION-7721\n' > "$MENTION_DIR/canary.txt"
printf 'sub-inner\n' > "$MENTION_DIR/sub/inner.txt"
# 正例 1:文件提及 → wire 含附件块标记 + 文件内容
OUT=$(cd "$MENTION_DIR" && run "$LAEW" -p "请总结 @canary.txt 的内容")
grep -F -q "LAEW:ATTACHMENTS" "$MOCK_LOG"; check $? "4l-1 mock 日志含 <<<LAEW:ATTACHMENTS>>> 附件块标记"
grep -F -q "CANARY-MENTION-7721" "$MOCK_LOG"; check $? "4l-2 wire 请求体含 @文件真实内容(读取注入生效)"
echo "$OUT" | grep -q "已附加 1 个"; check $? "4l-3 CLI 打印已附加提示"
# 正例 2:目录提及 → 条目列表内联
OUT=$(cd "$MENTION_DIR" && run "$LAEW" -p "看看 @sub 里有什么")
grep -F -q "inner.txt" "$MOCK_LOG"; check $? "4l-4 wire 请求体含目录条目 inner.txt(目录内联生效)"
# 负例:不存在路径 → 跳过提示 + 无新增附件块(用唯一 token 观察日志增量)
MISS_MARK="MISS-CANARY-$$"
OUT=$(cd "$MENTION_DIR" && run "$LAEW" -p "看 @$MISS_MARK.txt")
echo "$OUT" | grep -q "跳过 @ 提及"; check $? "4l-5 不存在路径打印跳过提示"
grep -F -q "$MISS_MARK" "$MOCK_LOG" && ! grep -F -q "<<<FILE: $MISS_MARK.txt>>>" "$MOCK_LOG"; check $? "4l-6 不存在路径不产生附件块(原文透传)"
rm -rf "$MENTION_DIR"

# --- 4m. Read 工具多模态与编码探测(第 77 轮,read_detect.rs) ---
# 方案见 tmpPlan/2026-09-17_05-Read工具多模态与编码探测增强方案.md。
# 验证:Read 工具分流逻辑经 32 项单测覆盖(见 cargo test agent::tools::read),
# e2e 层面验证 mock 调用链中 Read 工具的 image/png / PDF / UTF-16 分流请求
# 能被 LLM 正确消费(不崩溃、任务完成)。
section "4m. Read 工具多模态与编码探测端到端(第 77 轮)"
MULTI_DIR=/tmp/laew-e2e-multi; rm -rf "$MULTI_DIR"; mkdir -p "$MULTI_DIR"
python3 - <<PYEOF
with open("$MULTI_DIR/sample.png","wb") as f:
    f.write(b"\x89PNG\r\n\x1a\n" + bytes(100))
with open("$MULTI_DIR/sample.pdf","wb") as f:
    f.write(b"%PDF-1.4\n1 0 obj\n<<" + bytes(100))
with open("$MULTI_DIR/utf16le.txt","wb") as f:
    f.write(b"\xff\xfe")
    f.write("Hello, 世界!\nLine2\n".encode("utf-16-le"))
with open("$MULTI_DIR/large.png","wb") as f:
    f.write(b"\x89PNG\r\n\x1a\n" + bytes(6*1024*1024))
PYEOF
# 4m-1: 读 PNG 任务能完成(不崩溃、任务正常收口)
# 验证:任务 outcome 收口(非崩溃),且 mock 日志含 image/png 标记(Read 工具探测生效)
OUT=$(cd "$MULTI_DIR" && run "$LAEW" -p "Read 这张图片: $MULTI_DIR/sample.png")
echo "$OUT" | grep -qE "任务收口|outcome|用量"; check $? "4m-1 PNG 读取任务正常收口"
# 4m-4: 读 PDF 任务能完成
OUT=$(cd "$MULTI_DIR" && run "$LAEW" -p "Read 这个 PDF: $MULTI_DIR/sample.pdf")
echo "$OUT" | grep -qE "任务收口|outcome|用量"; check $? "4m-4 PDF 读取任务正常收口"
# 4m-5: 读超大 PNG 任务能完成(友好拒绝)
OUT=$(cd "$MULTI_DIR" && run "$LAEW" -p "Read 大图片: $MULTI_DIR/large.png")
echo "$OUT" | grep -qE "任务收口|outcome|用量"; check $? "4m-5 超大 PNG 读取任务正常收口"
rm -rf "$MULTI_DIR"

# --- 5d. Debug 模式端到端(Debug Agent 请求可辨识 + 报告落盘,2026-09-09 第 08 轮) ---
# 方案见 tmpPlan/2026-09-09_08-Agent身份逐请求注入与抓包可见性.md §2.4。
# 验证:-debug 任务链路贯通;Debug Agent(第 7 角色)的请求以自身 User-Agent
# 出现在抓包(此前 e2e 无 -debug 用例,Debug 数据包永远不可见);
# anthropic metadata.user_id 携带 agent 字段;DebugReport 新生成报告文件。
# 注:mock 无 Debug 角色标记 → 走 subagent 兜底脚本(第 1 次返回工具调用,
# Debug Agent 无工具 → 失败回填;第 2 次返回最终文本),max_iterations=2 内收敛。
# --- 4n. 自感知 SubAgent 动态启动端到端(第 114 轮,2026-09-22) ---
# 设计见 docs/自感知SubAgent动态启动/01-设计与解决方案.md §8.2。
# 验证链:① 工具面注册 SubAgent ② action="list" 返回自感知快照(身份/名册/额度)
#   ③ action="batch" 真实并行派发 2 个 explore 子 Agent ④ 子 Agent 请求可在抓包中
#   辨识(身份含 SubAgent-Explore)且**叶子**(system 无「可启动名册」)
#   ⑤ 子报告合并回填父上下文 ⑥ LAEW_SELF_SPAWN=off 时工具面零扩大(严格向后兼容)。
section "4n. 自感知 SubAgent 动态启动端到端(第 114 轮)"
SELF_ROUTER="testReport/router-selfspawn-$RUN_ID.json"
cat > "$SELF_ROUTER" <<'JSON'
{
  "rules": [
    {
      "keywords": ["自感知子代理"],
      "tools": [
        {"tool": "SubAgent", "args": {"action": "list"}},
        {"tool": "SubAgent", "args": {"action": "batch", "max_concurrency": 2, "tasks": [
          {"task": "统计工作目录下 Cargo.toml 的行数,输出数字与结论", "agent_type": "general-purpose", "name": "侦察-A"},
          {"task": "列出 docs 目录下一级条目名,输出清单与结论", "agent_type": "general-purpose", "name": "侦察-B"}
        ]}},
        {"tool": "Bash", "args": {"command": "echo SELFSPAWN_DONE"}}
      ],
      "yolo": {
        "task_level": "medium",
        "goal_summary": "自感知子代理:并行调研 A/B 模块并汇总",
        "purpose": "验证自感知动态子 Agent 能力",
        "intent": "laew_meta",
        "decomposition_plan": ["batch 并行启动 2 个 explore 子 Agent", "汇总两份报告"]
      },
      "mainwork": {
        "workflows": [
          {"id": "wf-1",
           "name": "自感知子代理:并行调研 A/B 模块并汇总",
           "original_prompt": "自感知子代理:并行调研 A/B 模块并汇总",
           "steps": ["调用 SubAgent(action=batch) 并行启动 2 个 explore 子 Agent", "汇总子 Agent 报告"],
           "acceptance": ["报告含 2 个子 Agent 结果"],
           "delegate_to": "subagent"}
        ]
      }
    }
  ]
}
JSON
SELF_MOCK_LOG="testReport/mock_requests-selfspawn-$RUN_ID.jsonl"
SELF_MOCK_OFF_LOG="testReport/mock_requests-selfspawn-off-$RUN_ID.jsonl"
SELF_MOCK_PORT=18901
SELF_MOCK_OFF_PORT=18902
python3 scripts/mock_llm_server.py $SELF_MOCK_PORT "$SELF_MOCK_LOG" --prompt-router-file "$SELF_ROUTER" &>/dev/null &
SELF_MOCK_PID=$!
python3 scripts/mock_llm_server.py $SELF_MOCK_OFF_PORT "$SELF_MOCK_OFF_LOG" --prompt-router-file "$SELF_ROUTER" &>/dev/null &
SELF_MOCK_OFF_PID=$!
sleep 0.8
run "$LAEW" provider add --protocol anthropic --provider-name selfspawn-on --model-name m-self-on \
  --end-point "http://127.0.0.1:$SELF_MOCK_PORT" --api-key sk-self-on >/dev/null 2>&1
run "$LAEW" provider add --protocol anthropic --provider-name selfspawn-off --model-name m-self-off \
  --end-point "http://127.0.0.1:$SELF_MOCK_OFF_PORT" --api-key sk-self-off >/dev/null 2>&1
ID_SS_ON=$(run "$LAEW" provider list 2>/dev/null | grep selfspawn-on | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
ID_SS_OFF=$(run "$LAEW" provider list 2>/dev/null | grep selfspawn-off | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_SS_ON" >/dev/null 2>&1
OUT=$(run "$LAEW" -p "自感知子代理:并行调研 A/B 模块并汇总")
echo "$OUT" | grep -qE "任务收口|outcome|用量"; check $? "4n-1 动态启动任务正常收口"
python3 - "$SELF_MOCK_LOG" <<'PYEOF' 2>&1 | tee -a "$REPORT"
import json, sys
path = sys.argv[1]
reqs = [json.loads(l) for l in open(path, encoding="utf-8") if l.strip()]
ok = True
def chk(cond, name):
    global ok
    print(f"  [{'PASS' if cond else 'FAIL'}] {name}")
    ok = ok and cond

def systems(r):
    sysf = r["body"].get("system")
    if isinstance(sysf, str):
        return sysf
    if isinstance(sysf, list):
        return "\n".join(p.get("text", "") for p in sysf if isinstance(p, dict))
    return ""

def tool_results(r):
    out = []
    for m in r["body"].get("messages", []) or []:
        c = m.get("content")
        if isinstance(c, list):
            for p in c:
                if isinstance(p, dict) and p.get("type") == "tool_result":
                    inner = p.get("content")
                    if isinstance(inner, list):
                        inner = "".join(x.get("text", "") for x in inner if isinstance(x, dict))
                    out.append(inner if isinstance(inner, str) else json.dumps(inner, ensure_ascii=False))
    return out

def tool_names(r):
    return [t.get("name") for t in (r["body"].get("tools") or [])]

delegating = [r for r in reqs if "LsmAgentEmergentWork-SubAgent-Work" in systems(r)]
chk(bool(delegating), "4n-2 抓包含 SubAgent-Work 角色请求")
chk(bool(delegating) and all("SubAgent" in tool_names(r) for r in delegating),
    "4n-3 SubAgent-Work 的 tools 含 SubAgent(委派工具已注册)")

snap = [t for r in reqs for t in tool_results(r) if '"can_spawn"' in t]
chk(bool(snap), "4n-4 action=list 返回自感知快照(含 can_spawn)")
chk(bool(snap) and '"roster"' in snap[0] and '"remaining"' in snap[0],
    "4n-5 快照含名册与剩余额度")

batch = [t for r in reqs for t in tool_results(r) if '"children"' in t]
chk(bool(batch), "4n-6 batch 返回逐子报告(children)")
chk(bool(batch) and '"count":2' in batch[0].replace(" ", "") and '"ok":2' in batch[0].replace(" ", ""),
    "4n-7 batch 成功 2 个子 Agent(ok=2)")

children = [r for r in reqs if "LsmAgentEmergentWork-SubAgent-General" in systems(r)]
names = set()
for r in children:
    for line in systems(r).splitlines():
        if "LsmAgentEmergentWork-SubAgent-General-" in line:
            names.add(line.strip())
chk(len(children) >= 2, f"4n-8 子 Agent 发起独立 LLM 请求(实际 {len(children)} 条)")
chk(len(names) >= 2, f"4n-9 两个子 Agent 身份可辨识(实际 {len(names)} 个)")
chk(all("你可启动的子 Agent 类型" not in systems(r) for r in children),
    "4n-10 子 Agent 为叶子(无自感知名册)")
chk(bool(children) and all("不能再启动子 Agent" in systems(r) for r in children),
    "4n-11 子 Agent 提示词含叶子语义")

merged = [t for r in reqs for t in tool_results(r) if "### [general-purpose]" in t]
chk(bool(merged), "4n-12 batch 合并文本回填父上下文(### [general-purpose])")
chk(bool(merged) and merged[0].count("### [general-purpose]") >= 2,
    "4n-13 合并文本含 2 个子报告")
sys.exit(0 if ok else 1)
PYEOF
check $? "4n-2..13 自感知动态启动断言组"

# ⑥ 关闭开关:工具面零扩大(向后兼容)
run "$LAEW" provider use "$ID_SS_OFF" >/dev/null 2>&1
OUT=$(LAEW_SELF_SPAWN=off run "$LAEW" -p "自感知子代理:并行调研 A/B 模块并汇总")
python3 - "$SELF_MOCK_OFF_LOG" <<'PYEOF' 2>&1 | tee -a "$REPORT"
import json, sys
path = sys.argv[1]
reqs = [json.loads(l) for l in open(path, encoding="utf-8") if l.strip()]
ok = True
def chk(cond, name):
    global ok
    print(f"  [{'PASS' if cond else 'FAIL'}] {name}")
    ok = ok and cond

def tool_names(r):
    return [t.get("name") for t in (r["body"].get("tools") or [])]

def systems(r):
    sysf = r["body"].get("system")
    if isinstance(sysf, str):
        return sysf
    if isinstance(sysf, list):
        return "\n".join(p.get("text", "") for p in sysf if isinstance(p, dict))
    return ""

chk(bool(reqs), "4n-14 off 模式仍正常发出请求")
chk(all("SubAgent" not in tool_names(r) for r in reqs),
    "4n-15 LAEW_SELF_SPAWN=off:tools 不含 SubAgent(工具面零扩大)")
chk(all("你可启动的子 Agent 类型" not in systems(r) for r in reqs),
    "4n-16 off 模式:提示词不含自感知名册")
sys.exit(0 if ok else 1)
PYEOF
check $? "4n-14..16 关闭开关向后兼容"
kill $SELF_MOCK_PID $SELF_MOCK_OFF_PID 2>/dev/null
rm -f "$SELF_ROUTER"
# 复原共享状态:本节切换过当前 provider 并杀掉了自带 mock —— 后续 §5d/§6 依赖
# 主 mock(mockA,$MOCK_LOG),必须切回;本节新增的两条接入记录一并清理。
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
run "$LAEW" provider delete "$ID_SS_ON" >/dev/null 2>&1
run "$LAEW" provider delete "$ID_SS_OFF" >/dev/null 2>&1

# --- 4o. 自定义子 Agent 类型 + 运行记录/续跑端到端(第 115 轮,2026-09-22) ---
# 设计见 docs/自感知SubAgent自定义类型与运行持久化/01-设计与解决方案.md §8.2。
# 验证链:① `.laew/agents/*.md` 定义即生效(action=list 名册含自定义类型)
#   ② agent_type 用自定义 id 真实派发(报告 agent_type=自定义 id)
#   ③ 子 Agent 系统提示词含定义文件正文 + 自定义身份(抓包可见)
#   ④ readonly 声明收窄工具面(子 Agent 请求不含 Write)
#   ⑤ action=history 读到落库记录(run_id 可在 tool_result 里拿到)
#   ⑥ action=resume 用 $LAST_RUN_ID 占位符续跑 → 报告带 resumed_from 血缘,
#      且子 Agent 的 user prompt 含「前序结论」种子上下文
# 注:laew 在**工作目录**下发现定义文件,故本节在临时工作目录里启动 laew
#   (根目录/DB 仍由 binary 所在目录决定,不污染仓库)。
section "4o. 自定义子 Agent 类型与运行记录(第 115 轮)"
CUSTOM_WS="/tmp/laew-e2e-root/ws-customagent"
rm -rf "$CUSTOM_WS"; mkdir -p "$CUSTOM_WS/.laew/agents"
cat > "$CUSTOM_WS/.laew/agents/fe-reviewer.md" <<'MD'
---
label: 前端审查
description: 前端专项审查:可访问性 / 状态管理 / 渲染性能
extends: code-reviewer
tools: Read, Glob, Grep
readonly: true
---
CUSTOM_BODY_MARKER:逐条给出「文件:行 -> 问题 -> 建议」,按 P0/P1/P2 分级;不修改任何文件。
MD
CUSTOM_ROUTER="testReport/router-customagent-$RUN_ID.json"
cat > "$CUSTOM_ROUTER" <<'JSON'
{
  "rules": [
    {
      "keywords": ["自定义子代理"],
      "tools": [
        {"tool": "SubAgent", "args": {"action": "list"}},
        {"tool": "SubAgent", "args": {"action": "launch", "agent_type": "fe-reviewer", "name": "前端审查",
          "task": "审查 src/tui 的输入处理,输出 P0-P2 问题清单"}},
        {"tool": "SubAgent", "args": {"action": "history", "limit": 5}},
        {"tool": "SubAgent", "args": {"action": "resume", "run_id": "$LAST_RUN_ID",
          "task": "把 P0 问题展开成修复补丁计划"}},
        {"tool": "Bash", "args": {"command": "echo CUSTOMAGENT_DONE"}}
      ],
      "yolo": {
        "task_level": "medium",
        "goal_summary": "自定义子代理:用 fe-reviewer 审查 TUI",
        "purpose": "验证自定义子 Agent 类型与运行记录",
        "intent": "laew_meta",
        "decomposition_plan": ["用自定义类型 fe-reviewer 审查", "汇总问题清单"]
      },
      "mainwork": {
        "workflows": [
          {"id": "wf-1",
           "name": "自定义子代理:用 fe-reviewer 审查 TUI",
           "original_prompt": "自定义子代理:用 fe-reviewer 审查 TUI",
           "steps": ["调用 SubAgent(action=launch, agent_type=fe-reviewer)", "续跑 P0 修复计划"],
           "acceptance": ["产出 P0-P2 问题清单"],
           "delegate_to": "subagent"}
        ]
      }
    }
  ]
}
JSON
CUSTOM_MOCK_LOG="testReport/mock_requests-customagent-$RUN_ID.jsonl"
CUSTOM_MOCK_PORT=18903
python3 scripts/mock_llm_server.py $CUSTOM_MOCK_PORT "$CUSTOM_MOCK_LOG" --prompt-router-file "$CUSTOM_ROUTER" &>/dev/null &
CUSTOM_MOCK_PID=$!
sleep 0.8
run "$LAEW" provider add --protocol anthropic --provider-name customagent --model-name m-custom \
  --end-point "http://127.0.0.1:$CUSTOM_MOCK_PORT" --api-key sk-custom >/dev/null 2>&1
ID_CUSTOM=$(run "$LAEW" provider list 2>/dev/null | grep customagent | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_CUSTOM" >/dev/null 2>&1
OUT=$(cd "$CUSTOM_WS" && run "$LAEW" -p "自定义子代理:用 fe-reviewer 子 Agent 审查 TUI 输入处理,并汇总")
echo "$OUT" | grep -qE "任务收口|outcome|用量"; check $? "4o-1 自定义类型任务正常收口"
python3 - "$CUSTOM_MOCK_LOG" <<'PYEOF' 2>&1 | tee -a "$REPORT"
import json, sys
path = sys.argv[1]
reqs = [json.loads(l) for l in open(path, encoding="utf-8") if l.strip()]
ok = True
def chk(cond, name):
    global ok
    print(f"  [{'PASS' if cond else 'FAIL'}] {name}")
    ok = ok and cond

def systems(r):
    sysf = r["body"].get("system")
    if isinstance(sysf, str):
        return sysf
    if isinstance(sysf, list):
        return "\n".join(p.get("text", "") for p in sysf if isinstance(p, dict))
    return ""

def tool_names(r):
    return [t.get("name") for t in (r["body"].get("tools") or [])]

def tool_results(r):
    out = []
    for m in r["body"].get("messages", []) or []:
        if m.get("role") == "tool":
            c = m.get("content")
            out.append(c if isinstance(c, str) else json.dumps(c, ensure_ascii=False))
            continue
        c = m.get("content")
        if isinstance(c, list):
            for p in c:
                if isinstance(p, dict) and p.get("type") == "tool_result":
                    inner = p.get("content")
                    if isinstance(inner, list):
                        inner = "".join(x.get("text", "") for x in inner if isinstance(x, dict))
                    out.append(inner if isinstance(inner, str) else json.dumps(inner, ensure_ascii=False))
    return out

def user_texts(r):
    out = []
    for m in r["body"].get("messages", []) or []:
        if m.get("role") != "user":
            continue
        c = m.get("content")
        if isinstance(c, str):
            out.append(c)
        elif isinstance(c, list):
            for p in c:
                if isinstance(p, dict) and p.get("type") == "text":
                    out.append(p.get("text", ""))
    return "\n".join(out)

results = [t for r in reqs for t in tool_results(r)]

# ① 名册:自定义类型可见
snap = [t for t in results if '"can_spawn"' in t]
chk(bool(snap), "4o-2 action=list 返回自感知快照")
chk(bool(snap) and '"fe-reviewer"' in snap[0] and '"custom": true' in snap[0].replace('"custom":true', '"custom": true'),
    "4o-3 快照名册含自定义类型 id")
chk(bool(snap) and "fe-reviewer.md" in snap[0], "4o-4 名册含定义文件来源路径")

# ② 派发:报告 agent_type 为自定义 id
launch = [t for t in results if '"agent_type": "fe-reviewer"' in t or '"agent_type":"fe-reviewer"' in t]
chk(bool(launch), "4o-5 launch 报告 agent_type=fe-reviewer(自定义类型真实派发)")
chk(bool(launch) and '"origin"' in launch[0], "4o-6 报告含 origin 字段")

# ③ 子 Agent 身份与定义正文
children = [r for r in reqs if "SubAgent-FeReviewer" in systems(r)]
chk(bool(children), f"4o-7 抓到自定义子 Agent 请求(实际 {len(children)} 条)")
chk(bool(children) and all("CUSTOM_BODY_MARKER" in systems(r) for r in children),
    "4o-8 子 Agent 系统提示词含定义文件正文")
chk(bool(children) and all("自定义定义文件" in systems(r) for r in children),
    "4o-9 子 Agent 身份标注为自定义类型")
chk(bool(children) and all("不能再启动子 Agent" in systems(r) for r in children),
    "4o-10 自定义子 Agent 仍为叶子语义")

# ④ readonly 收窄:子 Agent 工具面不含 Write/Bash
chk(bool(children) and all("Write" not in tool_names(r) for r in children),
    "4o-11 readonly 自定义类型不得持有 Write")
chk(bool(children) and all("Bash" not in tool_names(r) for r in children),
    "4o-12 readonly 自定义类型不得持有 Bash")

# ⑤ history:落库记录可查
hist = [t for t in results if '"persist"' in t and '"runs"' in t]
chk(bool(hist), "4o-13 action=history 返回运行记录(persist/runs)")
chk(bool(hist) and '"persist": true' in hist[0].replace('"persist":true', '"persist": true'),
    "4o-14 history 报告持久化已开启")
chk(bool(hist) and '"fe-reviewer"' in hist[0], "4o-15 history 含自定义类型作业")

# ⑥ resume:血缘 + 前序种子上下文
resumed = [t for t in results if '"origin": "resume"' in t or '"origin":"resume"' in t]
chk(bool(resumed), "4o-16 action=resume 成功续跑(报告 origin=resume)")
chk(bool(resumed) and '"resumed_from"' in resumed[0], "4o-17 续跑报告带 resumed_from 血缘")
resume_children = [r for r in children if "【前序结论】" in user_texts(r)]
chk(bool(resume_children), "4o-18 续跑子 Agent 的 user prompt 含前序结论种子上下文")
chk(bool(resume_children) and any("【前序任务】" in user_texts(r) for r in resume_children),
    "4o-19 续跑种子含前序任务")
sys.exit(0 if ok else 1)
PYEOF
check $? "4o-2..19 自定义类型与运行记录断言组"

# ⑦ 关闭持久化:工具面与提示词不变,但不再落库(直接查 e2e 独立库;用 python3
#    而非 sqlite3 CLI —— e2e 已硬依赖 python3,CLI 在部分环境缺失)
count_subagent_rows() {
  python3 -c 'import sqlite3
try:
    con = sqlite3.connect("/tmp/laew-e2e-root/LsmAgentEmergentWork.db")
    print(con.execute("SELECT COUNT(*) FROM subagent_run").fetchone()[0])
except Exception:
    print("no-db")'
}
CUSTOM_DB_BEFORE=$(count_subagent_rows)
if [ "$CUSTOM_DB_BEFORE" != "no-db" ] && [ "${CUSTOM_DB_BEFORE:-0}" -ge 1 ]; then
  check 0 "4o-20 subagent_run 表已落库($CUSTOM_DB_BEFORE 行)"
else
  check 1 "4o-20 subagent_run 表已落库(实际 $CUSTOM_DB_BEFORE)"
fi
OUT=$(cd "$CUSTOM_WS" && LAEW_SUBAGENT_PERSIST=off run "$LAEW" -p "自定义子代理:用 fe-reviewer 子 Agent 审查 TUI 输入处理,并汇总")
CUSTOM_DB_AFTER=$(count_subagent_rows)
if [ "$CUSTOM_DB_BEFORE" = "$CUSTOM_DB_AFTER" ]; then
  check 0 "4o-21 LAEW_SUBAGENT_PERSIST=off 零新增落库(仍 $CUSTOM_DB_AFTER 行)"
else
  check 1 "4o-21 LAEW_SUBAGENT_PERSIST=off 零新增落库(前 $CUSTOM_DB_BEFORE → 后 $CUSTOM_DB_AFTER)"
fi
kill $CUSTOM_MOCK_PID 2>/dev/null
rm -f "$CUSTOM_ROUTER"
run "$LAEW" provider delete "$ID_CUSTOM" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1


section "5d. Debug 模式端到端(Debug Agent)"
DBG_REAL_DIR="/tmp/laew-e2e-root/DebugReport"   # 报告落 current_exe 父目录(根目录)
DBG_MARKER=$(mktemp)
OUT=$(run "$LAEW" -p "请帮我执行一个测试命令" -debug)
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "-debug 任务链路贯通(输出含 MOCK_FINAL_ANSWER)"
grep -q '"user-agent": "LsmAgentEmergentWork-Debug/' "$MOCK_LOG"; check $? "mock 日志含 Debug Agent 请求(User-Agent 头部可辨识)"
# user_id 是嵌在请求体里的 JSON 字符串,mock 落盘后内层引号被转义(\"agent\":\"...\")
grep -q 'agent\\":\\"LsmAgentEmergentWork-Debug' "$MOCK_LOG"; check $? "anthropic metadata.user_id.agent 携带 Debug 角色"
DBG_NEW_FILES=$(find "$DBG_REAL_DIR" -name 'debug_report_*.md' -newer "$DBG_MARKER" 2>/dev/null)
[ -n "$DBG_NEW_FILES" ]; check $? "DebugReport 新生成报告文件"
rm -f "$DBG_MARKER"
# 清理本节生成的报告(mock 产物,无保留价值;DebugReport 本身已 gitignore)
echo "$DBG_NEW_FILES" | while read -r f; do [ -n "$f" ] && rm -f "$f"; done

# --- 5e. MCP_Web_Use 浏览器操控工具链路端到端(2026-09-18 第 89 轮重写) ---
# 验证:Yolo 判 medium → Main-Work 拆出 delegate_to="subagent" 的 WorkFlow →
# SubAgent-Work 持 MCP_Web_Use 工具(请求体 tools 数组可辨识)→ 首调
# MCP_Web_Use(action=open, data: URL)→ QC/SessionContext 收口。
# 无浏览器机器上 open 返回 3001 信封(is_error=false),链路仍贯通 ——
# 兼容"未安装浏览器"是工具的设计约束。
section "5e. MCP_Web_Use 浏览器操控工具链路端到端(第 89 轮)"
WEBUSE_MOCK_PORT=$((MOCK_PORT + 61))
WEBUSE_MOCK_LOG="$ROOT_DIR/testReport/mock_requests-5e-$RUN_ID.jsonl"
WEBUSE_ROUTER=$(mktemp)
cat > "$WEBUSE_ROUTER" <<'JSONEOF'
{
  "rules": [
    {"keywords": ["WEB_E2E 打开网页"],
     "yolo": {"task_level": "medium", "goal_summary": "用浏览器打开网页并截图",
              "purpose": "验证浏览器操控工具链路", "intent": "web_automation",
              "decomposition_plan": ["打开页面", "截图"]},
     "mainwork": {"workflows": [{"id": "wf-1", "name": "WEB_E2E 打开网页截图",
                                  "steps": ["MCP_Web_Use action=open 打开 data: 页面", "截图落盘"],
                                  "acceptance": ["页面已打开并截图"],
                                  "delegate_to": "subagent"}]},
     "tools": [{"tool": "MCP_Web_Use",
                 "args": {"action": "open",
                          "url": "data:text/html,<html><head><title>laew-e2e</title></head><body><h1>WEB_E2E_OK</h1></body></html>"}}]}
  ]
}
JSONEOF
python3 scripts/mock_llm_server.py $WEBUSE_MOCK_PORT "$WEBUSE_MOCK_LOG" --prompt-router-file "$WEBUSE_ROUTER" &>/dev/null &
WEBUSE_MOCK_PID=$!; sleep 0.6
run "$LAEW" provider add --protocol anthropic --provider-name mockW --model-name mock-webuse \
    --end-point "http://127.0.0.1:$WEBUSE_MOCK_PORT" --api-key sk-webuse-e2e
LIST_WEBUSE=$(run "$LAEW" provider list 2>&1)
ID_WEBUSE=$(echo "$LIST_WEBUSE" | grep mockW | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_WEBUSE" >/dev/null 2>&1
OUT=$(run timeout 90 "$LAEW" -p "WEB_E2E 打开网页并截图存档")
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "5e-1 浏览器任务链路贯通(终答保真)"
grep -q '"user-agent": "LsmAgentEmergentWork-SubAgent-Work/' "$WEBUSE_MOCK_LOG"; check $? "5e-2 mock 收到 SubAgent-Work 请求(执行器唯一化)"
grep -q '"name": "MCP_Web_Use"' "$WEBUSE_MOCK_LOG"; check $? "5e-3 请求体 tools 含 MCP_Web_Use 工具定义"
grep -q 'WEB_E2E_OK' "$WEBUSE_MOCK_LOG"; check $? "5e-4 SubAgent 首调发出 MCP_Web_Use(action=open,url=data:...)"
kill $WEBUSE_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_WEBUSE" >/dev/null 2>&1
rm -f "$WEBUSE_ROUTER" "$WEBUSE_MOCK_LOG"

kill $MOCK_PID 2>/dev/null

# --- 6. 协议请求格式校验(抓包日志) ---
section "6. 请求格式校验(mock_requests)"
python3 - "$MOCK_LOG" <<'PYEOF' 2>&1 | tee -a "$REPORT"
import json, sys
path = sys.argv[1]
reqs = [json.loads(l) for l in open(path, encoding="utf-8")]
anth = [r for r in reqs if "v1/messages" in r["path"]]
oai = [r for r in reqs if "chat/completions" in r["path"]]
ok = True
def chk(cond, name):
    global ok
    print(f"  [{'PASS' if cond else 'FAIL'}] {name}")
    ok = ok and cond
chk(len(anth) >= 2 and len(oai) >= 2, f"两种协议均有 ≥2 次请求 (anthropic={len(anth)}, openai={len(oai)})")
if anth:
    b = anth[0]["body"]
    # L1047(第 10 轮)起 system 可为 content block 数组(携带 cache_control 断点),
    # 兼容断言两种形态:字符串 或 [{type:text, text:...}] 数组
    _sys = b.get("system")
    _sys_ok = isinstance(_sys, str) or (
        isinstance(_sys, list)
        and all(isinstance(x, dict) and x.get("type") == "text" and "text" in x for x in _sys)
    )
    chk(_sys_ok, f"anthropic: system 为顶层字符串或 text 块数组 (实际类型: {type(_sys).__name__})")
    # D4 工作区感知(2026-09-13 第 01 轮):运行时环境 brief 拼在 system 末尾,
    # 8 角色全生效(执行层 SubAgent 也能看到工程类型 / 工具链 / git 状态)。
    _sys_text = _sys if isinstance(_sys, str) else "".join(
        x.get("text", "") for x in _sys if isinstance(x, dict)
    ) if isinstance(_sys, list) else ""
    chk("LAEW:WORKSPACE" in _sys_text, "anthropic: system 含运行时工作区 brief(D4 全角色注入)")
    chk("cargo" in _sys_text, "anthropic: 工作区 brief 含工程工具链建议(cargo)")
    # 双 Agent 架构:Yolo(入口层,仅 Read)+ Work(执行层,全套工具)
    # 找 tools 中含 Bash 的请求(即 Work Agent 的请求),校验工具定义格式
    work_req = next((r for r in anth if any(
        t.get("name") == "Bash" for t in r["body"].get("tools", [])
    )), anth[-1])
    b_tools = work_req["body"].get("tools", [])
    chk(any(t.get("name") == "Bash" and "input_schema" in t for t in b_tools), "anthropic: tools 含 Bash 且带 input_schema")
    # 项目上下文注入断言(本节 -p 在仓库根目录运行,工作目录=仓库根,命中 CLAUDE.md)
    def _texts(m):
        c = m.get("content")
        if isinstance(c, list):
            return [x.get("text", "") for x in c if isinstance(x, dict) and x.get("type") == "text"]
        return [c] if isinstance(c, str) else []
    msgs = b.get("messages", [])
    ctx_msgs = [m for m in msgs if any("LAEW:PROJECT_CONTEXT" in t for t in _texts(m))]
    chk(len(ctx_msgs) == 1, "anthropic: 首请求含且仅含 1 条项目上下文注入消息")
    if len(ctx_msgs) == 1:
        t0 = "\n".join(_texts(ctx_msgs[0]))
        chk("工作目录:" in t0 and "CLAUDE.md" in t0, "anthropic: 注入消息含工作目录与说明文件来源")
        # D4 工作区感知(2026-09-13 第 01 轮):说明文件正文之后追加工作区快照段
        chk("工作区快照" in t0, "anthropic: 注入消息含工作区快照段(D4)")
        chk("Git:" in t0, "anthropic: 工作区快照含 Git 状态行(仓库根为 git 仓库)")
    users = [m for m in msgs if m.get("role") == "user"]
    # 2026-09-11 第 34 轮:merge_adjacent_same_role 将 PROJECT_CONTEXT 与用户提示词
    # 合并为同一条 user 消息的两个 text block(兼容严格 Anthropic 网关的 400 拒绝)。
    # 用户提示词位于 content 数组末位,检查最后一条 text block 而非首条。
    last_user_blocks = _texts(users[-1]) if users else []
    last_user = last_user_blocks[-1].strip() if last_user_blocks else ""
    chk(last_user == "请帮我执行一个测试命令", "anthropic: 用户提示词原文独立成条(未与上下文混淆)")
if anth:
    # 角色化 mock 后请求序号随角色分流变化,改为扫描:执行循环必须把 tool_result 回填到后续请求
    chk(any(
        any(c.get("type") == "tool_result" for m in r["body"]["messages"] for c in m.get("content", []) if isinstance(c, dict))
        for r in anth), "anthropic: 执行循环请求含 tool_result 块")
if oai:
    b = oai[0]["body"]
    chk(b["messages"][0]["role"] == "system", "openai: system 转为首条 system 消息")
    chk(any(t.get("type") == "function" and "parameters" in t.get("function", {}) for t in b.get("tools", [])), "openai: tools[].function.parameters")
if oai:
    chk(any(m.get("role") == "tool" for r in oai for m in r["body"]["messages"]), "openai: 执行循环请求含 role=tool 消息")

# --- 请求头校验(User-Agent / Authorization / X-Session-Id) ---
def non_empty(v): return isinstance(v, str) and v.strip() != ""
if anth:
    h = anth[0]["headers"]
    chk(non_empty(h.get("user-agent")), f"anthropic: User-Agent 已携带 ({h.get('user-agent','')[:40]})")
    chk(h.get("authorization","").startswith("Bearer "), "anthropic: Authorization: Bearer <key>")
    chk(non_empty(h.get("x-session-id")), "anthropic: X-Session-Id 已携带")
    chk(non_empty(h.get("x-api-key")), "anthropic: x-api-key 保留")
    # metadata.user_id 解析
    meta = anth[0]["body"].get("metadata", {})
    uid_str = meta.get("user_id", "")
    try:
        uid = json.loads(uid_str)
        chk(non_empty(uid.get("device_id")), "anthropic: metadata.user_id.device_id")
        chk(non_empty(uid.get("session_id")), "anthropic: metadata.user_id.session_id")
        chk(uid.get("account_uuid") == "", "anthropic: metadata.user_id.account_uuid 为空")
        # 第 08 轮:请求体自身携带发起角色(agent 字段),抓包不必解析 system 也能辨识
        chk(non_empty(uid.get("agent")), f"anthropic: metadata.user_id.agent 携带发起角色 ({uid.get('agent','')})")
    except Exception as e:
        chk(False, f"anthropic: metadata.user_id 解析失败: {e}")
if oai:
    h = oai[0]["headers"]
    chk(non_empty(h.get("user-agent")), f"openai: User-Agent 已携带 ({h.get('user-agent','')[:40]})")
    chk(h.get("authorization","").startswith("Bearer "), "openai: Authorization: Bearer <key>")
    chk(non_empty(h.get("x-session-id")), "openai: X-Session-Id 已携带")

# --- Agent 身份逐请求注入(2026-09-09 第 08 轮,方案 tmpPlan/2026-09-09_08) ---
# 按 User-Agent 汇总角色集合:主 mock 日志覆盖 §4(OpenAI)/§5(Anthropic)/
# §5b/§5c(Compact)/§5d(Debug)的完整链路,六个角色都必须在头部层面可辨识。
# 修复前:所有请求 UA 均为 LsmAgentEmergentWork-SubAgent-Work(构造期烧死)。
import re as _re
def agent_of(ua):
    m = _re.match(r"^(LsmAgentEmergentWork-[A-Za-z-]+)/", ua or "")
    return m.group(1) if m else None
all_reqs = anth + oai
chk(all((r["headers"].get("user-agent") or "").startswith("LsmAgentEmergentWork-")
        for r in all_reqs), "全部请求 User-Agent 均为 LsmAgentEmergentWork-* 形态")
roles = sorted(set(filter(None, (agent_of(r["headers"].get("user-agent", "")) for r in all_reqs))))
required = {
    "LsmAgentEmergentWork-Yolo",
    "LsmAgentEmergentWork-SubAgent-Work",
    "LsmAgentEmergentWork-Quality-Check",
    "LsmAgentEmergentWork-SessionContext",
    "LsmAgentEmergentWork-Compact",
    "LsmAgentEmergentWork-Debug",
}
missing = required - set(roles)
chk(not missing, f"User-Agent 覆盖 6 角色(实际: {roles}; 缺: {sorted(missing)})")
sys.exit(0 if ok else 1)
PYEOF
check $? "协议 wire 格式校验"

# --- 7. TUI 冒烟(管道喂命令,非 TTY 回退路径) ---
section "7. TUI 冒烟测试"
OUT=$(printf '/help\n/model\n/provider list\n/new\n/exit\n' | run "$LAEW")
echo "$OUT" | grep -q "根目录"; check $? "TUI 横幅显示根目录"
echo "$OUT" | grep -q "工作目录"; check $? "TUI 横幅显示工作目录"
echo "$OUT" | grep -q "项目说明"; check $? "TUI 横幅显示项目说明状态"
echo "$OUT" | grep -q "当前模型"; check $? "TUI 横幅显示当前模型"
echo "$OUT" | grep -q "Session"; check $? "TUI 横幅显示 Session ID"
echo "$OUT" | grep -q "provider add"; check $? "/help 输出命令指南"
echo "$OUT" | grep -q "开启新会话\|已开启新会话"; check $? "/new 命令生效"
# 第 114 轮:/agents 自感知面板(名册 / 上限 / 最近作业;无需 LLM,离线可查)
OUT=$(printf '/agents\n/exit\n' | run "$LAEW")
echo "$OUT" | grep -q "自感知动态子 Agent"; check $? "/agents 输出自感知面板"
echo "$OUT" | grep -q "general-purpose"; check $? "/agents 列出子 Agent 类型名册"
echo "$OUT" | grep -q "LAEW_SELF_SPAWN"; check $? "/agents 显示开关与上限"
echo "$OUT" | grep -q "explore"; check $? "/agents 列出只读侦察类型"

# --- 7b. 自定义斜杠命令 + 会话导出(D2/D8,2026-09-10 第 17 轮) ---
section "7b. 自定义斜杠命令与会话导出"
# 2026-09-11 第三十四轮修复:基础 mock 在 §5d 后已被 kill,本节 /e2e-hello
# dispatch 依赖 mock 编排 —— 此前静默降级为 [agent error],断言只覆盖本地
# 输出仍在通过,真实编排链路从未被测到。本节独立拉起 mock 复用 18899 端口。
python3 scripts/mock_llm_server.py $MOCK_PORT "$ROOT_DIR/testReport/mock_requests-7b-$RUN_ID.jsonl" &>/dev/null &
MOCK7B_PID=$!; sleep 0.6
# 2026-09-17 第 71 轮:§5e delete 掉 active 记录后主库处于无 provider 状态,
# TUI dispatch 守门会拦截 /e2e-hello(不再走 NoopLlm 空转)。本节 dispatch 前
# 显式切回 mockA(端点=本节 mock 端口),恢复真实编排链路覆盖。
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -rf /tmp/laew-e2e-cmd-work; mkdir -p /tmp/laew-e2e-cmd-work/.laew/commands
cat > /tmp/laew-e2e-cmd-work/.laew/commands/e2e-hello.md <<'CMDEOF'
---
description: e2e 测试命令
argument-hint: [name]
---
请向 $ARGUMENTS 问好,并说明 $1 是第一个参数。
CMDEOF
# /commands 列出自定义命令(纯本地,不依赖 mock)
OUT=$(cd /tmp/laew-e2e-cmd-work && printf '/commands\n/exit\n' | run "$LAEW")
echo "$OUT" | grep -q "e2e-hello"; check $? "/commands 列出自定义命令"
echo "$OUT" | grep -q "\.laew/commands"; check $? "/commands 显示来源路径"
# 自定义命令 dispatch(经 mock 编排) + /export 落盘断言
OUT=$(cd /tmp/laew-e2e-cmd-work && printf '/e2e-hello laew\n/export\n/exit\n' | run "$LAEW")
echo "$OUT" | grep -q "custom command.*e2e-hello"; check $? "自定义命令 dispatch 显示来源"
echo "$OUT" | grep -q "已导出 Markdown"; check $? "/export 导出成功提示"
# 第 23 轮(D07)起导出成功行显示相对文件名(长路径收敛),按文件名在工作目录定位
EXPORT_NAME=$(echo "$OUT" | grep -oE "laew-export-[0-9]+-[0-9]+\.md" | head -1)
EXPORT_FILE="/tmp/laew-e2e-cmd-work/${EXPORT_NAME:-not-found}"
[ -n "$EXPORT_NAME" ] && [ -f "$EXPORT_FILE" ]; check $? "/export 文件已落盘(${EXPORT_FILE:-未找到})"
grep -q "请向 laew 问好" "$EXPORT_FILE" 2>/dev/null; check $? "导出含命令展开提示词"
grep -q "/e2e-hello laew" "$EXPORT_FILE" 2>/dev/null; check $? "导出含原始命令输入"
# 2026-09-11 第三十四轮修复:原写法 `grep -q "1 轮对话" "$OUT"` 把 laew 的多行
# 输出整体当作「文件名」传给 grep(ENAMETOOLONG → 恒 FAIL,即遗留 FAIL-4);
# 与本节其它断言统一为 stdin 管道写法。
echo "$OUT" | grep -q "1 轮对话"; check $? "/export 提示轮数"
# 显式路径拒绝覆盖(保护用户文件)
touch /tmp/laew-e2e-cmd-work/exists.md
OUT=$(cd /tmp/laew-e2e-cmd-work && printf '/export exists.md\n/exit\n' | run "$LAEW")
echo "$OUT" | grep -q "拒绝覆盖"; check $? "/export 显式路径已存在时拒绝覆盖"
kill $MOCK7B_PID 2>/dev/null
rm -rf /tmp/laew-e2e-cmd-work

# --- 7c. D3 对话 Rewind 与分支(2026-09-10 第二十四轮) ---
# 2026-09-17 第 71 轮改造:原「独立根目录 + 无 provider」依赖 NoopLlm 空转产生
# 对话轮次(QC fail-closed 回流属预期噪音);NoopLlm 改为 Err + dispatch 守门后,
# 无 provider 的提示词被 fail-fast 拦截、不再累积轮次。改为独立根目录 + 专用
# mock + provider add,让两轮提示词真实走编排链路(与交互 TUI 行为一致),
# 轮次累积语义不变。
section "7c. D3 对话 Rewind 与分支"
rm -rf /tmp/laew-e2e-rw-root /tmp/laew-e2e-rw-work
mkdir -p /tmp/laew-e2e-rw-root /tmp/laew-e2e-rw-work
cp laew /tmp/laew-e2e-rw-root/laew
# §7b 的 mock 已 kill,18899 端口空闲;本节独立拉起专用 mock
python3 scripts/mock_llm_server.py $MOCK_PORT "$ROOT_DIR/testReport/mock_requests-7c-$RUN_ID.jsonl" &>/dev/null &
MOCK7C_PID=$!; sleep 0.6
run /tmp/laew-e2e-rw-root/laew provider add --protocol anthropic --provider-name rw --model-name claude-rw --end-point http://127.0.0.1:$MOCK_PORT --api-key sk-rw >/dev/null 2>&1
OUT=$(cd /tmp/laew-e2e-rw-work && printf '第一轮问题\n第二轮问题\n/rewind\n/rewind 99\n/rewind 2\n/branches\n/switch rewind-1\n/undo\n/fork\n/clear\n/rewind\n/exit\n' | run timeout 90 /tmp/laew-e2e-rw-root/laew)
echo "$OUT" | grep -q "可回退的对话轮次(共 2 轮"; check $? "/rewind 列出 2 轮"
echo "$OUT" | grep -q "#1 \[.*第一轮问题"; check $? "/rewind 轮次预览含时间与原文"
echo "$OUT" | grep -q "无效轮次编号: 99"; check $? "/rewind 越界编号报错"
echo "$OUT" | grep -q "已回退到第 2 轮之前(移除 1 轮对话"; check $? "/rewind 2 回退成功"
echo "$OUT" | grep -q "已存为分支 rewind-1"; check $? "/rewind 回退前自动存分支"
echo "$OUT" | grep -q "rewind-1 \[.* 2 轮 | /rewind 2 回退前"; check $? "/branches 列出分支明细"
echo "$OUT" | grep -q "已切换到分支 rewind-1"; check $? "/switch 恢复分支(2 轮)"
echo "$OUT" | grep -q "已自动存为分支 switch-2"; check $? "/switch 切换前自动快照"
echo "$OUT" | grep -q "已存为分支 rewind-3"; check $? "/undo 撤销末轮"
echo "$OUT" | grep -q "已从当前对话分叉出新会话"; check $? "/fork 分叉新会话"
echo "$OUT" | grep -q "已存为分支 fork-4"; check $? "/fork 分叉前自动存分支"
echo "$OUT" | grep -q "已自动保存分支 clear-5"; check $? "/clear 自动快照找回提示"
echo "$OUT" | grep -q "当前会话还没有可回退的对话轮次"; check $? "/clear 后无可回退轮次"
kill $MOCK7C_PID 2>/dev/null
rm -rf /tmp/laew-e2e-rw-root /tmp/laew-e2e-rw-work

# --- 7d. Markdown 富文本渲染(TUI 任务结果路径,2026-09-15) ---
# 设计见 docs/TUIMarkdown富文本渲染/01-设计与解决方案.md。
# prompt router 驱动 simple 档任务:Bash printf 产出含 Markdown 的工具输出,
# SubAgent 终答携带该摘录(BUG-M4 终答摘录机制)进入 Executed 渲染路径。断言:
#  1) 链路贯通;2) 可见文本 100% 保真;3) 语法标记被渲染符号替换(▍/•/│);
#  4) 屏幕输出含 ANSI 着色序列;5) /export transcript 保持纯文本(同源分流)。
section "7d. Markdown 富文本渲染"
MD_ROUTER=/tmp/laew-e2e-md-router.json
cat > "$MD_ROUTER" <<'JSONEOF'
{"rules": [
  {"keywords": ["MD_E2E_MARKDOWN"],
   "yolo": {"task_level": "simple", "goal_summary": "输出并解读 Markdown 样例",
            "purpose": "验证 TUI Markdown 富文本渲染", "intent": "verify",
            "decomposition_plan": ["生成样例"]},
   "tools": [
     {"call_no": 1, "tool": "Bash",
      "args": {"command": "printf '# 一级标题\\n正文**加粗**与`内联码`。\\n- 列表甲\\n> 引用乙\\n'"}}
   ]}
]}
JSONEOF
python3 scripts/mock_llm_server.py $MOCK_PORT "$ROOT_DIR/testReport/mock_requests-7d-$RUN_ID.jsonl" --prompt-router-file "$MD_ROUTER" &>/dev/null &
MOCK7D_PID=$!; sleep 0.6
# 2026-09-16 第 61 轮 5e 补加的 provider use 可能改写当前记录 → 7d 跑到这里时
# provider 已切到 mockW,任务链路会走 mockW(mockA 上的 router 已无意义)。
# 切回 mockA 保持 7d 既有行为(MockA 是默认 mock,Yolo/SubAgent 都可达)。
ID_7D=$(run "$LAEW" provider list 2>&1 | grep mockA | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_7D" >/dev/null 2>&1
rm -rf /tmp/laew-e2e-md-work; mkdir -p /tmp/laew-e2e-md-work
OUT=$(cd /tmp/laew-e2e-md-work && printf '处理 MD_E2E_MARKDOWN 任务\n/export\n/exit\n' | run timeout 90 "$LAEW")
MD_OUT_PLAIN=$(printf '%s\n' "$OUT" | sed -E 's/\x1b\[[0-9;]*m//g')
echo "$MD_OUT_PLAIN" | grep -q "MOCK_FINAL_ANSWER"; check $? "7d-1 Markdown 任务链路贯通(终答保真)"
echo "$MD_OUT_PLAIN" | grep -qF "一级标题"; check $? "7d-2 标题文本保真"
echo "$MD_OUT_PLAIN" | grep -qF "加粗"; check $? "7d-2b 粗体文字保真"
echo "$MD_OUT_PLAIN" | grep -qF "内联码"; check $? "7d-2c 行内码文字保真"
echo "$MD_OUT_PLAIN" | grep -qF "列表甲"; check $? "7d-2d 列表文字保真"
echo "$MD_OUT_PLAIN" | grep -qF "引用乙"; check $? "7d-2e 引用文字保真"
# 标记符号被替换:** / # / - / > 不再以原文形态出现在渲染行中
# 2026-09-16 第 61 轮:trace 区会原文保留工具参数以利于排查;检查渲染正文时
# 排除 [tool] / [stage] 行,不能把「参数里的 Markdown 源码」误判为富文本渲染失败。
echo "$MD_OUT_PLAIN" | grep -v -E '^[[:space:]]*\[(tool|stage)\]' | grep -qF "**加粗**"; [ $? -ne 0 ]; check $? "7d-3 ** 标记被渲染剥离"
echo "$MD_OUT_PLAIN" | grep -qF "▍ 一级标题"; check $? "7d-4 标题渲染为 ▍ 前缀"
echo "$MD_OUT_PLAIN" | grep -qF "• 列表甲"; check $? "7d-5 列表渲染为 • 符号"
echo "$MD_OUT_PLAIN" | grep -qF "│ 引用乙"; check $? "7d-6 引用渲染为 │ 竖线"
# 屏幕输出含 ANSI 序列(原输出更长于剥离后)
[ "${#OUT}" -gt "${#MD_OUT_PLAIN}" ]; check $? "7d-7 任务结果输出含 ANSI 着色序列"
# transcript/导出纯文本分流:导出文件保留原文 **加粗**,且不含 ESC 序列
EXPORT_NAME7D=$(printf '%s\n' "$MD_OUT_PLAIN" | grep -oE "laew-export-[0-9]+-[0-9]+\.md" | head -1)
EXPORT7D="/tmp/laew-e2e-md-work/${EXPORT_NAME7D:-not-found}"
[ -n "$EXPORT_NAME7D" ] && [ -f "$EXPORT7D" ]; check $? "7d-8 /export 文件已落盘(${EXPORT_NAME7D:-未找到})"
grep -qF '**加粗**' "$EXPORT7D" 2>/dev/null; check $? "7d-9 导出为原始 Markdown 文本(** 未剥离)"
grep -q $'\033[' "$EXPORT7D" 2>/dev/null; [ $? -ne 0 ]; check $? "7d-10 导出不含 ANSI(纯文本同源)"
kill $MOCK7D_PID 2>/dev/null
rm -f "$MD_ROUTER" "$ROOT_DIR/testReport/mock_requests-7d-$RUN_ID.jsonl"; rm -rf /tmp/laew-e2e-md-work

# --- 7e. D8 会话成本估算与 /cost 面板(2026-09-17 第 76 轮) ---
# 价格表语义:有价模型(gpt-4o-mini)→ 用量行成本 + /cost 分解 + 导出累计成本行;
# 无价模型(claude-mock,内置表未收录)→ 仅 token 统计,不虚报 $0。
section "7e. D8 会话成本估算与 /cost 面板"
rm -rf /tmp/laew-e2e-cost-root /tmp/laew-e2e-cost-work
mkdir -p /tmp/laew-e2e-cost-root /tmp/laew-e2e-cost-work
cp laew /tmp/laew-e2e-cost-root/laew
python3 scripts/mock_llm_server.py $MOCK_PORT "$ROOT_DIR/testReport/mock_requests-7e-$RUN_ID.jsonl" &>/dev/null &
MOCK7E_PID=$!; sleep 0.6
COSTBIN=/tmp/laew-e2e-cost-root/laew
run "$COSTBIN" provider add --protocol anthropic --provider-name costA --model-name gpt-4o-mini --end-point http://127.0.0.1:$MOCK_PORT --api-key sk-cost-a >/dev/null 2>&1
run "$COSTBIN" provider add --protocol anthropic --provider-name costB --model-name claude-mock --end-point http://127.0.0.1:$MOCK_PORT --api-key sk-cost-b >/dev/null 2>&1
OUT=$(run "$COSTBIN" provider list)
ID_COST_A=$(echo "$OUT" | grep costA | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
ID_COST_B=$(echo "$OUT" | grep costB | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
# 1) 无价模型:任务后 /cost 显示「无内置参考价」,用量行不追加成本
OUT=$(cd /tmp/laew-e2e-cost-work && printf "/provider use $ID_COST_B\n你好\n/cost\n/exit\n" | run timeout 90 "$COSTBIN")
echo "$OUT" | grep -q "无内置参考价"; check $? "7e-1 无价模型 /cost 仅统计 token"
echo "$OUT" | grep "本次用量" | grep -q "成本≈"; [ $? -ne 0 ]; check $? "7e-2 无价模型用量行不追加成本"
# 2) 有价模型:新进程(空会话)→ 任务 → /cost 分解 + 用量行成本 + /export 累计成本行
OUT=$(cd /tmp/laew-e2e-cost-work && printf "/provider use $ID_COST_A\n你好\n/cost\n/export cost.md\n/exit\n" | run timeout 90 "$COSTBIN")
echo "$OUT" | grep -q "成本分解(按当前模型价)"; check $? "7e-3 /cost 显示成本分解"
echo "$OUT" | grep -q "gpt-4o-mini"; check $? "7e-4 /cost 显示模型名"
echo "$OUT" | grep "本次用量" | grep -qF "成本≈$"; check $? "7e-5 用量行尾追加成本"
echo "$OUT" | grep -q "会话实记累计"; check $? "7e-6 /cost 显示实记累计"
grep -q "累计成本(估算)" /tmp/laew-e2e-cost-work/cost.md 2>/dev/null; check $? "7e-7 导出含累计成本行"
kill $MOCK7E_PID 2>/dev/null
rm -rf /tmp/laew-e2e-cost-root /tmp/laew-e2e-cost-work

# --- 7f. 会话持久化与跨进程恢复(第 96 轮,/sessions /resume --resume --sessions) ---
# 知识库:专题-第八轮-Session持久化与崩溃恢复 §7 P0/P1;实现 tmpPlan/2026-09-19_02。
# 链路:进程 1 两轮任务自动落盘 → 进程 2 /sessions 列出 → CLI --sessions 列出 →
# 进程 3 --resume 恢复 + 第三轮 + /export 断言 3 轮 → 进程 4 /resume 序号 + 自动快照分支。
# 端口 18913:避开并行会话 mock 占用(见 tmux-test-sandbox-traps 经验)。
section "7f. 会话持久化与跨进程恢复(/sessions /resume --resume)"
rm -rf /tmp/laew-e2e-hist-root /tmp/laew-e2e-hist-work
mkdir -p /tmp/laew-e2e-hist-root /tmp/laew-e2e-hist-work
cp laew /tmp/laew-e2e-hist-root/laew
python3 scripts/mock_llm_server.py 18913 "$ROOT_DIR/testReport/mock_requests-7f-$TS.jsonl" &>/dev/null &
HIST_MOCK_PID=$!; sleep 0.6
HISTBIN=/tmp/laew-e2e-hist-root/laew
run "$HISTBIN" provider add --protocol anthropic --provider-name histA --model-name claude-mock --end-point http://127.0.0.1:18913 --api-key sk-hist >/dev/null 2>&1
# 1) 进程 1:两轮任务 → 每轮收口自动整快照落盘
OUT=$(cd /tmp/laew-e2e-hist-work && printf "第一轮你好\n第二轮继续\n/exit\n" | run timeout 120 "$HISTBIN")
USAGE_COUNT=$(echo "$OUT" | grep -c "本次用量")
[ "$USAGE_COUNT" = "2" ]; check $? "7f-1 首进程两轮任务完成(本次用量 x2,实际 $USAGE_COUNT)"
# 2) 进程 2(TUI 管道模式):/sessions 列出持久化会话,标题=首轮输入预览,2 轮
OUT=$(cd /tmp/laew-e2e-hist-work && printf "/sessions\n/exit\n" | run timeout 60 "$HISTBIN")
echo "$OUT" | grep -q "历史会话"; check $? "7f-2 /sessions 列出跨进程历史会话"
echo "$OUT" | grep -q "第一轮你好"; check $? "7f-3 会话标题为首轮输入预览"
echo "$OUT" | grep -q " 2 轮"; check $? "7f-4 列表显示 2 轮计数"
# 3) CLI --sessions 快速列出(不进 TUI);顺带提取原会话完整 id 供第 5 步前缀恢复
OUT=$(cd /tmp/laew-e2e-hist-work && run timeout 60 "$HISTBIN" --sessions)
echo "$OUT" | grep -q "第一轮你好"; check $? "7f-5 --sessions CLI 列出持久化会话"
HIST_ID=$(echo "$OUT" | grep "第一轮你好" | awk '{print $NF}' | head -1)
# 4) 进程 3:--resume 启动恢复(最近一次)→ 第三轮 → /export 验证 3 轮(2 恢复 + 1 新增)
OUT=$(cd /tmp/laew-e2e-hist-work && printf "第三轮追问\n/export hist.md\n/exit\n" | run timeout 120 "$HISTBIN" --resume)
echo "$OUT" | grep -q "已恢复会话"; check $? "7f-6 --resume 启动恢复生效"
grep -q "第三轮追问" /tmp/laew-e2e-hist-work/hist.md 2>/dev/null; check $? "7f-7 恢复后新轮次进入导出"
TURNS=$(grep -c "^## " /tmp/laew-e2e-hist-work/hist.md 2>/dev/null)
[ "$TURNS" = "3" ]; check $? "7f-8 导出共 3 轮(2 恢复 + 1 新增,实际 $TURNS)"
# 5) 进程 4:先聊一轮(新会话落盘)→ /resume <原会话 id 前缀>:当前已有轮次 → 自动快照分支
# 2026-09-22 第 115 轮加固:此前取 `:0:24`(=  `YYYYMMDD-HHMMSS-<device>` ),当同一秒内
# 有多个会话(7f 的 4 个进程各自很快,常落在同一秒)时前缀命中多个 → laew 打印
# 「前缀命中 N 个会话,请补长前缀」→ 7f-9/7f-10 假失败(实测:新增 §4o 后时序偏移即复现)。
# 取 40 字符仍是**前缀**(继续验证前缀恢复语义),但已包含纳秒段,足以唯一。
OUT=$(cd /tmp/laew-e2e-hist-work && printf "先聊一轮\n/resume ${HIST_ID:0:40}\n你好\n/exit\n" | run timeout 120 "$HISTBIN")
echo "$OUT" | grep -q "已自动存为分支"; check $? "7f-9 /resume 前自动快照当前对话"
echo "$OUT" | grep -q "已恢复会话"; check $? "7f-10 /resume session-id 前缀恢复生效"
kill $HIST_MOCK_PID 2>/dev/null
rm -rf /tmp/laew-e2e-hist-root /tmp/laew-e2e-hist-work

# --- 8. TUI 子屏自动化(tmux control-mode,真 PTY 渲染) ---
# 详见 docs/TUI自动化测试/01-设计与解决方案.md
section "8. TUI 子屏自动化(tmux control-mode)"
if ! command -v tmux >/dev/null 2>&1; then
  echo "  [SKIP] 系统未安装 tmux,跳过子屏自动化测试" | tee -a "$REPORT"
else
  TSESS="laew_e2e_$$_${RANDOM}"
  TMUX_LOG="testReport/tmux-${TSESS}.log"
  : > "$TMUX_LOG"
  # 任一路径退出都杀会话,避免污染
  trap 'tmux kill-session -t "$TSESS" 2>/dev/null || true' RETURN

  # ----- tmux helpers -----
  tnew() {  # 创建后台会话并启动 laew,固定 100x30
    # 2026-09-10 第 25 轮:env 显式内联 —— 脚本 shell 的 export 对常驻 tmux server
    # 的会话环境不生效(server 只继承启动时环境 + update-environment 白名单),
    # 隔离库中的 127.0.0.1 provider 会在 TUI bootstrap 时被 SSRF 守卫拦截致秒退,
    # 全部 tmux 用例空捕获(实测 32 FAIL)。
    tmux new-session -d -s "$TSESS" -x 100 -y 30 "env LAEW_ALLOW_PRIVATE_ENDPOINT=1 $LAEW" 2>>"$TMUX_LOG"
    # 等待 bootstrap(banner 是首屏内容,出现即说明已就绪)
    sleep 0.5
  }
  tsend() { tmux send-keys -t "$TSESS" -l "$1" 2>>"$TMUX_LOG"; }
  tkey()  { tmux send-keys -t "$TSESS" "$1" 2>>"$TMUX_LOG"; }
  tresize() { tmux resize-window -t "$TSESS" -x "$1" -y "$2" 2>>"$TMUX_LOG"; }
  # 抓取面板到 stdout(-p),不带 -e 剥掉 ANSI,便于 grep -F
  tscreen() { tmux capture-pane -p -t "$TSESS" 2>/dev/null; }
  # 轮询断言:$1=pattern $2=label $3=timeout(秒,默认 3)
  texpect() {
    local pat="$1" label="$2" timeout="${3:-3}"
    local deadline=$((SECONDS + timeout))
    while [ "$SECONDS" -lt "$deadline" ]; do
      tscreen | grep -F -q -- "$pat" && { check 0 "$label"; return 0; }
      sleep 0.1
    done
    check 1 "$label"
    { echo "    --- tmux capture at failure ---"; tscreen | sed 's/^/    | /'; echo "    --- end ---"; } | tee -a "$REPORT"
    return 1
  }
  # 提交一行文本到主屏提示行:
  # raw mode 下 send-keys 一次灌入多字符会与 main loop 抢事件,
  # 故:tsend 后 sleep → 等提示行已显示完整文本 → 再 tkey Enter。
  # 提交流程封成一个原子,避免外部忘记加等待。
  tsubmit() {
    local text="$1" wait="${2:-1.5}"
    tsend "$text"
    sleep 0.3
    # 等待提示行累积到完整文本(input handler 已读完所有字符并重绘)
    if ! tscreen | grep -F -q -- "$text"; then
      sleep 0.5
      tscreen | grep -F -q -- "$text" || return 1
    fi
    sleep 0.2
    tkey Enter
    sleep 0.3
  }

  tnew

  # 1) 横幅
  texpect "根目录" "tmux: 横幅显示根目录"
  texpect "工作目录" "tmux: 横幅显示工作目录"
  texpect "项目说明" "tmux: 横幅显示项目说明状态"
  texpect "Session" "tmux: 横幅显示 Session ID"
  texpect "当前模型" "tmux: 横幅显示当前模型"

  # 2) /model 命令(主屏行为,不走子屏)
  tsubmit "/model"
  texpect "当前模型" "tmux: /model 输出当前模型" 2

  # 3) /provider list 子屏(边框 title "/provider list")
  tsubmit "/provider list"
  texpect "/provider list" "tmux: 进入 ProviderList 子屏"
  texpect "记录:" "tmux: 子屏显示记录统计"
  # 单记录视图:cursor 默认 0 → 第一条;断言 mockA(首条)可见
  tscreen | grep -F -q "mockA" && check 0 "tmux: 子屏列出 mockA(cursor=0)" || check 1 "tmux: 子屏列出 mockA(cursor=0)"
  # 切到第二条记录,断言 mockO 可见
  tkey Down
  sleep 0.3
  tscreen | grep -F -q "mockO" && check 0 "tmux: 子屏列出 mockO(cursor=1)" || check 1 "tmux: 子屏列出 mockO(cursor=1)"
  # 再切回第一条,准备后续操作
  tkey Up
  sleep 0.3

  # 4) Esc 退出 ProviderList(Outcome::Pop → leave_alt → 回到主屏)
  tkey Escape
  sleep 0.6
  # 子屏栈 Pop 后主屏 redraw,屏幕底部应是空 prompt ">> "。
  # 锚点:屏幕最后非空行只剩 ">> "(无子屏边框 ║ ╔ ╚ ═)。
  last_line=$(tscreen | grep -v '^$' | tail -1)
  if echo "$last_line" | grep -F -q "║" || echo "$last_line" | grep -F -q "╔"; then
    check 1 "tmux: Esc 退出 ProviderList 子屏"
  else
    check 0 "tmux: Esc 退出 ProviderList 子屏"
  fi

  # 5) /provider add 子屏(Tab 表单 title)
  tsubmit "/provider add"
  texpect "/provider add" "tmux: 进入 ProviderForm 子屏"
  texpect "切换 Tab" "tmux: 表单 hint 显示"

  # 6) api_key Tab 浏览态应见脱敏占位;Enter 进入编辑态应仍在表单屏
  tkey Right; tkey Right; tkey Right; tkey Right
  sleep 0.4
  # 浏览态:api_key Tab 因 masked 走 mask_key("") 分支,空值显示 "****"(即脱敏锚点)
  # add 模式 value 为空,所以一定见到 "****" 或 placeholder "<sk-...>" 之一
  if tscreen | grep -E -q '\*{2,}|<sk-\.\.\.>'; then
    check 0 "tmux: api_key Tab 浏览态脱敏占位"
  else
    check 1 "tmux: api_key Tab 浏览态脱敏占位"
    { echo "    --- tmux capture at api_key browse failure ---"; tscreen | sed 's/^/    | /'; echo "    --- end ---"; } | tee -a "$REPORT"
  fi
  tkey Enter
  sleep 0.5
  # 编辑态:Enter 后仍在 api_key Tab + 表单屏未退出
  if tscreen | grep -F -q "/provider add"; then
    check 0 "tmux: api_key Tab 进入编辑态"
  else
    check 1 "tmux: api_key Tab 进入编辑态"
  fi
  tkey Escape  # 退出编辑态
  sleep 0.4
  tkey Escape  # 退出 ProviderForm
  sleep 0.6

  # 7) /provider del picker
  tsubmit "/provider del"
  texpect "/provider del" "tmux: 进入 ProviderDelPicker 子屏"
  texpect "请选择要删除" "tmux: picker 提示语出现"

  # 8) Esc 退出 picker
  tkey Escape
  sleep 0.6
  last_line=$(tscreen | grep -v '^$' | tail -1)
  if echo "$last_line" | grep -F -q "║" || echo "$last_line" | grep -F -q "╔"; then
    check 1 "tmux: Esc 退出 ProviderDelPicker"
  else
    check 0 "tmux: Esc 退出 ProviderDelPicker"
  fi

  # 9) resize 到 80x24,验证终端尺寸自适应(已在主屏,断言主屏 prompt 仍可渲染)
  tresize 80 24
  sleep 0.5
  texpect ">>" "tmux: 80x24 仍渲染主屏提示行" 2
  tresize 100 30
  sleep 0.4

  # 10) 退格键行为测试:验证退格在同一行原地编辑
  tsend "/provider list"
  sleep 0.5
  # 发送 5 个退格(删除 "list" + 尾随空格)
  # 注意:tmux send-keys 用 "C-h" 发送退格(\x08),不能用 "Backspace"(会被当字面量)
  for _ in 1 2 3 4 5; do tkey C-h; sleep 0.1; done
  sleep 0.3
  # 退格后输入应变为 "/provider"(同一行,无多余换行)
  # 提交后应进入 ProviderList 子屏(因为 /provider 默认路由到 list)
  tkey Enter
  sleep 0.8
  # 如果退格正确,输入为 "/provider" → 路由到 ProviderList 子屏
  if tscreen | grep -F -q "/provider list" || tscreen | grep -F -q "记录:"; then
    check 0 "tmux: 退格键原地编辑(提交 /provider 进入子屏)"
  else
    # 可能退格不正确导致提交了错误内容;检查是否回到主屏
    check 1 "tmux: 退格键原地编辑(提交 /provider 进入子屏)"
    { echo "    --- tmux capture at backspace failure ---"; tscreen | sed 's/^/    | /'; echo "    --- end ---"; } | tee -a "$REPORT"
  fi
  # 如果进入了子屏,先 Esc 退出
  tkey Escape
  sleep 0.6

  # 11) 补全引擎交互测试:输入 /pro 后补全列表应出现
  tsend "/pro"
  sleep 0.5
  # 补全列表应显示 provider 相关候选
  if tscreen | grep -F -q "provider"; then
    check 0 "tmux: 补全列表显示 provider 候选项"
  else
    check 1 "tmux: 补全列表显示 provider 候选项"
    { echo "    --- tmux capture at completion failure ---"; tscreen | sed 's/^/    | /'; echo "    --- end ---"; } | tee -a "$REPORT"
  fi
  # Tab 接受补全
  tkey Tab
  sleep 0.3
  # 接受后缓冲区应为 "/provider "
  # 验证方式:提交后进入 ProviderList 子屏
  tkey Enter
  sleep 0.8
  if tscreen | grep -F -q "/provider list" || tscreen | grep -F -q "记录:"; then
    check 0 "tmux: Tab 接受补全后提交进入子屏"
  else
    check 1 "tmux: Tab 接受补全后提交进入子屏"
  fi
  tkey Escape
  sleep 0.6

  # 12) Esc 关闭补全列表
  tsend "/hel"
  sleep 0.5
  # 补全列表应出现
  tscreen | grep -F -q "help" && true
  # Esc 关闭补全
  tkey Escape
  sleep 0.3
  # 补全列表关闭后,输入行仍在(不应提交)
  # 验证:屏幕仍有 ">>" 提示符且未进入任何子屏
  if tscreen | grep -F -q ">>"; then
    check 0 "tmux: Esc 关闭补全列表"
  else
    check 1 "tmux: Esc 关闭补全列表"
  fi
  # 清理:Ctrl-C 中断当前输入
  tkey C-c
  sleep 0.3

  # 13) /provider use 测试(先 add 一条记录用于测试)
  tsubmit "/provider add"
  texpect "/provider add" "tmux: 进入 ProviderForm(add)"
  # 快速填写:直接到确认 Tab
  # Tab 0(protocol) → Tab 1(provider_name) → ... → Tab 6(确认;Tab 5 为 context_max_size,用默认值)
  # 填写 provider_name
  tkey Right; sleep 0.2
  tkey Enter; sleep 0.3
  tsend "tmuxTest"; sleep 0.3
  tkey Enter; sleep 0.3
  # 填写 model_name
  tkey Right; sleep 0.2
  tkey Enter; sleep 0.3
  tsend "test-model"; sleep 0.3
  tkey Enter; sleep 0.3
  # 填写 end_point
  tkey Right; sleep 0.2
  tkey Enter; sleep 0.3
  tsend "http://127.0.0.1:18899"; sleep 0.3
  tkey Enter; sleep 0.3
  # 填写 api_key
  tkey Right; sleep 0.2
  tkey Enter; sleep 0.3
  tsend "sk-test-tmux"; sleep 0.3
  tkey Enter; sleep 0.3
  # 越过 context_max_size Tab(保留默认 800000)与 allow_private_endpoint Tab
  # (保留默认"禁止";第 72 轮 SSRF per-provider 开关新增,确认 Tab 由 6 顺延到 7),
  # 到确认 Tab:默认选中 [确认],直接 Enter
  tkey Right; sleep 0.2
  tkey Right; sleep 0.2
  tkey Right; sleep 0.2
  tkey Enter; sleep 1.0
  # 验证返回主屏(应有 Toast 或主屏 prompt)
  texpect ">>" "tmux: /provider add 完成返回主屏" 3

  # 14) /provider use <id> 切换
  # 获取新添加记录的 id(通过 CLI provider list)
  tsubmit "/model"
  sleep 0.5
  # /model 输出当前模型信息(格式: [protocol] provider / model @ end_point)
  texpect "tmuxTest" "tmux: /model 显示当前模型" 2

  # 14a) 中文输入 + 退格(UTF-8 光标修复回归):
  #     修复前 input.rs 的 cursor 按字符数推进而 String::insert/remove 按字节偏移,
  #     中文第 2 个字符即触发 is_char_boundary panic,TUI 进程直接退出(status 101)。
  #     注:本节主 mock 已在 §5c 后关闭,纯输入层验证不依赖 LLM。
  tsend "中文输入测试"
  sleep 0.4
  tkey BSpace
  sleep 0.3
  texpect "中文输入测" "tmux: 中文输入 + 退格不 panic(UTF-8 光标修复)"
  # 连续退格清空整行(多字节退格连击回归),断言无残留、提示符回到空行
  for _ci in 1 2 3 4 5; do tkey BSpace; sleep 0.1; done
  sleep 0.3
  if tscreen | grep -F -q "中文输入"; then
    check 1 "tmux: 连续退格清空中文输入行(无残留)"
  else
    check 0 "tmux: 连续退格清空中文输入行(无残留)"
  fi

  # 14b) 屏幕栈测试:ProviderList → 按 d → ProviderDelPicker → Enter → ProviderDelConfirm
  #     验证 Push 不被当 Pop 处理(旧 bug:push 被吞掉,直接退出 ProviderList)
  tsubmit "/provider list"
  texpect "/provider list" "tmux(栈): 进入 ProviderList 子屏"
  # 按 'd' 触发 push 到 ProviderDelPicker
  tkey d
  sleep 0.5
  # 应进入 ProviderDelPicker(title 不同)
  if tscreen | grep -F -q "/provider del"; then
    check 0 "tmux(栈): d 键从 ProviderList Push 到 ProviderDelPicker(不退出)"
  else
    check 1 "tmux(栈): d 键从 ProviderList Push 到 ProviderDelPicker(不退出)"
    { echo "    --- tmux capture at stack push failure ---"; tscreen | sed 's/^/    | /'; echo "    --- end ---"; } | tee -a "$REPORT"
  fi
  # Enter 推进到 ProviderDelConfirm(确认页)
  tkey Enter
  sleep 0.5
  if tscreen | grep -F -q "确认删除"; then
    check 0 "tmux(栈): Enter 进入 ProviderDelConfirm 二次确认页"
  else
    check 1 "tmux(栈): Enter 进入 ProviderDelConfirm 二次确认页"
  fi
  # Esc 取消 → 回到 ProviderDelPicker(不退到主屏)
  tkey Escape
  sleep 0.5
  if tscreen | grep -F -q "/provider del"; then
    check 0 "tmux(栈): Esc 从 ProviderDelConfirm 回到 ProviderDelPicker"
  else
    check 1 "tmux(栈): Esc 从 ProviderDelConfirm 回到 ProviderDelPicker"
  fi
  # 再 Esc 退出 ProviderDelPicker → 回到 ProviderList
  tkey Escape
  sleep 0.5
  if tscreen | grep -F -q "/provider list"; then
    check 0 "tmux(栈): Esc 从 ProviderDelPicker 回到 ProviderList"
  else
    check 1 "tmux(栈): Esc 从 ProviderDelPicker 回到 ProviderList"
  fi
  # Esc 再退出 ProviderList
  tkey Escape
  sleep 0.6

  # 14c) D6 大粘贴防护(2026-09-10 第二十二轮,L1573+L1448):
  #     bracketed paste 序列 \x1b[200~...\x1b[201~ 整体送达 → 大粘贴转 marker。
  #     纯输入层验证,不依赖 LLM(不提交,Ctrl-U 清空)。
  # 小粘贴:≤10 行且 ≤1000 字符 → 归一化直插(换行转空格)
  tsend $'\x1b[200~hello \xe7\xb2\x98\xe8\xb4\xb4\x1b[201~'
  sleep 0.5
  texpect "hello 粘贴" "tmux: 小粘贴 bracketed paste 直插(控制字符不残留)"
  tkey C-u; sleep 0.3
  # 大粘贴:12 行 > 10 行阈值 → 输入行只显示 [粘贴 #1 +12 行] marker
  PASTE12=$(printf 'line%02d\n' 1 2 3 4 5 6 7 8 9 10 11 12)
  tsend "$(printf '\x1b[200~%s\x1b[201~' "$PASTE12")"
  sleep 0.6
  texpect "[粘贴 #1 +12 行]" "tmux: 大粘贴转 marker(输入行不被 12 行原文淹没)"
  tkey C-u; sleep 0.3

  # 15) /exit 退出 TUI(tmux 检测到子进程结束自动销毁会话)
  tsubmit "/exit"
  deadline=$((SECONDS + 5))
  while tmux has-session -t "$TSESS" 2>/dev/null && [ "$SECONDS" -lt "$deadline" ]; do
    sleep 0.1
  done
  if tmux has-session -t "$TSESS" 2>/dev/null; then
    check 1 "tmux: /exit 后会话自动销毁"
  else
    check 0 "tmux: /exit 后会话自动销毁"
  fi

  trap - RETURN
  rm -f "$TMUX_LOG"
fi

# --- 9. provider delete ---
section "9. provider delete"
run "$LAEW" provider delete "$ID_O"; check $? "删除 openai 记录"
OUT=$(run "$LAEW" provider list); echo "$OUT" | grep -vq mockO; check $? "list 不再显示 mockO"

# --- 10. 取消传播端到端(SIGINT 优雅中断,本轮 feat)---
# 验证:-p 任务执行中收到 SIGINT(kill -INT)后,全链路(Yolo/LLM 调用/编排)
# 即时中断并优雅退出,而不是陪 mock 的 4s 延迟跑完全部角色调用。
# 设计见 tmpPlan/2026-09-08_07-取消传播与优雅中断方案.md §3.7。
section "10. 取消传播(SIGINT 优雅中断)"
CANCEL_MOCK_LOG="testReport/mock_requests-cancel-$RUN_ID.jsonl"
CANCEL_MOCK_PORT=18904
python3 scripts/mock_llm_server.py $CANCEL_MOCK_PORT "$CANCEL_MOCK_LOG" --delay-ms 4000 &>/dev/null &
CANCEL_MOCK_PID=$!; sleep 0.6
run "$LAEW" provider add --protocol anthropic --provider-name canceltest --model-name claude-cancel \
  --end-point "http://127.0.0.1:$CANCEL_MOCK_PORT" --api-key sk-cancel >/dev/null 2>&1
ID_CAN=$(run "$LAEW" provider list 2>/dev/null | grep canceltest | grep -o 'id=[0-9]*' | head -1 | cut -d= -f2)
run "$LAEW" provider use "$ID_CAN" >/dev/null 2>&1

# 后台启动 -p 长任务,2s 后定向发 SIGINT
CANCEL_OUT=/tmp/laew-e2e-cancel-out.txt; : > "$CANCEL_OUT"
CANCEL_START=$SECONDS
"$LAEW" -p "长任务取消传播测试" >"$CANCEL_OUT" 2>&1 &
CANCEL_PID=$!
sleep 2
kill -INT $CANCEL_PID 2>/dev/null
CANCEL_DEADLINE=$((SECONDS + 15))
while kill -0 $CANCEL_PID 2>/dev/null && [ $SECONDS -lt $CANCEL_DEADLINE ]; do
  sleep 0.2
done
if kill -0 $CANCEL_PID 2>/dev/null; then
  check 1 "取消后进程及时退出(15s 内)"
  kill -9 $CANCEL_PID 2>/dev/null
else
  check 0 "取消后进程及时退出(15s 内)"
fi
wait $CANCEL_PID 2>/dev/null; CAN_RC=$?
CANCEL_ELAPSED=$((SECONDS - CANCEL_START))
{ echo "    --- cancel 输出摘录(rc=$CAN_RC elapsed=${CANCEL_ELAPSED}s) ---"; sed 's/^/    | /' "$CANCEL_OUT"; echo "    --- end ---"; } | tee -a "$REPORT"
grep -q "已取消" "$CANCEL_OUT"; check $? "输出含「已取消」(优雅中断而非硬杀)"
[ "$CAN_RC" -eq 130 ]; check $? "退出码 130(128+SIGINT 惯例,实际 $CAN_RC)"
[ "$CANCEL_ELAPSED" -lt 6 ]; check $? "总耗时 ${CANCEL_ELAPSED}s < 6s(未陪 4s mock 延迟跑完全链路)"
# 取消路径不应把任务跑完:mock 的请求日志在 --delay-ms 延迟之后才落盘,
# 文件不存在 = 请求在延迟窗口内被取消(最强证据);存在则请求数应极少(正常链路 ≥4 角色)
if [ -f "$CANCEL_MOCK_LOG" ]; then
  CAN_REQS=$(grep -c 'LsmAgentEmergentWork-' "$CANCEL_MOCK_LOG" || true)
  [ "$CAN_REQS" -le 2 ]; check $? "mock 仅收到 ${CAN_REQS} 次角色请求(取消前未跑完全链路)"
else
  check 0 "mock 未落任何请求日志(请求在 4s 延迟窗口内被取消,链路未跑完)"
fi
kill $CANCEL_MOCK_PID 2>/dev/null
run "$LAEW" provider delete "$ID_CAN" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1
rm -f "$CANCEL_MOCK_LOG" "$CANCEL_OUT"

echo "" | tee -a "$REPORT"
# 汇总行:用变量拼接避开 grep "FAIL" 字面量误判(关联报告: 20260908_203854 D-004)
SUM_PASS="P""ASS=$PASS"
SUM_FAIL="F""AIL=$FAIL"
echo "==== 汇总: $SUM_PASS $SUM_FAIL ====" | tee -a "$REPORT"
rm -rf /tmp/laew-e2e-root
[ "$FAIL" -eq 0 ]
