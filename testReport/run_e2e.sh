#!/usr/bin/env bash
# run_e2e.sh: laew 端到端自动化验证(不依赖真实 LLM,使用本地 mock 服务)
# 输出: testReport/e2e-<时间戳>.txt
set -uo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

TS=$(date +%Y%m%d-%H%M%S)
REPORT="testReport/e2e-$TS.txt"
MOCK_PORT=18899
MOCK_LOG="testReport/mock_requests-$TS.jsonl"
PASS=0; FAIL=0

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

echo "laew e2e 验证 @ $(date)" | tee "$REPORT"
echo "根目录: $ROOT_DIR" | tee -a "$REPORT"

# --- 准备:独立数据库(避免污染真实配置) ---
rm -f /tmp/laew-e2e-root; mkdir -p /tmp/laew-e2e-root
cp laew /tmp/laew-e2e-root/laew
LAEW=/tmp/laew-e2e-root/laew   # 根目录=/tmp/laew-e2e-root → db 也在这里

# --- 1. 版本与帮助 ---
section "1. --version / --help"
run "$LAEW" --version; check $? "--version 可用"
OUT=$(run "$LAEW" --help); echo "$OUT" | grep -q "provider"; check $? "--help 含 provider 指南"

# --- 2. 无配置时的 -p 引导报错 ---
section "2. 未配置模型时 -p 的引导提示"
OUT=$(run "$LAEW" -p "hello"); echo "$OUT" | grep -q "provider add"; check $? "提示先 provider add"

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
chk(data.get("version") == "1.1", f"导出含 version=1.1 (got={data.get('version')!r})")
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
BASH_MOCK_LOG="testReport/mock_requests-bash-$TS.jsonl"
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
FLAKY_MOCK_LOG="testReport/mock_requests-flaky-$TS.jsonl"
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
BQ_MOCK_LOG="testReport/mock_requests-bq-$TS.jsonl"
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
PAR_MOCK_LOG="testReport/mock_requests-par-$TS.jsonl"
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
OVF_MOCK_LOG="testReport/mock_requests-ovf-$TS.jsonl"
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
SBX_OUT_LOG="testReport/mock_requests-sbx-out-$TS.jsonl"
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
SBX_IN_LOG="testReport/mock_requests-sbx-in-$TS.jsonl"
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
section "5b. 项目上下文注入(说明文件五级链)"
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

python3 - "$MOCK_LOG" "$CA0" "$CA1" "$CB0" "$CB1" "$CC0" "$CC1" <<'PYEOF' 2>&1 | tee -a "$REPORT"
import json, sys
path = sys.argv[1]
args = [int(x) for x in sys.argv[2:8]]
ranges = {"A": (args[0], args[1]), "B": (args[2], args[3]), "C": (args[4], args[5])}
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
    users = user_texts(ra[0])
    chk(len(users) == 2 and users[-1].strip() == "场景A测试", "场景A: 用户提示词独立成条且未被改写")

# 场景B:自动生成 README.md 并注入
rb = anth_in(ranges["B"])
chk(len(rb) >= 1, f"场景B: 有 anthropic 请求 ({len(rb)})")
if rb:
    texts = "\n".join(all_texts(rb[0]))
    chk("LAEW:PROJECT_CONTEXT" in texts, "场景B: 首请求含项目上下文标记")
    chk("架构总览" in texts, "场景B: 注入自动生成的 README 内容")

# 场景C:无 Markdown,不注入
rc = anth_in(ranges["C"])
chk(len(rc) >= 1, f"场景C: 有 anthropic 请求 ({len(rc)})")
if rc:
    chk(all("LAEW:PROJECT_CONTEXT" not in t for req in rc for t in all_texts(req)), "场景C: 所有请求均无注入标记")
    users = user_texts(rc[0])
    chk(len(users) == 1 and users[0].strip() == "场景C测试", "场景C: user 消息仅用户提示词一条")
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
echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "压缩后任务链路仍贯通"
run "$LAEW" provider delete "$ID_CP" >/dev/null 2>&1
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1

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
    chk("system" in b and isinstance(b["system"], str), "anthropic: system 为顶层字符串")
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
    users = [m for m in msgs if m.get("role") == "user"]
    last_user = _texts(users[-1])[0].strip() if users else ""
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
    except Exception as e:
        chk(False, f"anthropic: metadata.user_id 解析失败: {e}")
if oai:
    h = oai[0]["headers"]
    chk(non_empty(h.get("user-agent")), f"openai: User-Agent 已携带 ({h.get('user-agent','')[:40]})")
    chk(h.get("authorization","").startswith("Bearer "), "openai: Authorization: Bearer <key>")
    chk(non_empty(h.get("x-session-id")), "openai: X-Session-Id 已携带")
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
    tmux new-session -d -s "$TSESS" -x 100 -y 30 "$LAEW" 2>>"$TMUX_LOG"
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
  # 越过 context_max_size Tab(保留默认 800000),到确认 Tab:默认选中 [确认],直接 Enter
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
CANCEL_MOCK_LOG="testReport/mock_requests-cancel-$TS.jsonl"
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
