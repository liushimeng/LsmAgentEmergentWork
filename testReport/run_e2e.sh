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

# --- 4. mock LLM + OpenAI 协议端到端 ---
section "4. OpenAI 协议端到端(工具调用循环)"
python3 scripts/mock_llm_server.py $MOCK_PORT "$MOCK_LOG" &>/dev/null &
MOCK_PID=$!; sleep 0.6
OUT=$(run "$LAEW" -p "请帮我执行一个测试命令"); echo "$OUT" | grep -q "MOCK_FINAL_ANSWER"; check $? "返回最终文本"
run "$LAEW" provider use "$ID_A" >/dev/null 2>&1

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
if len(anth) >= 2:
    b2 = anth[1]["body"]
    chk(any(c.get("type") == "tool_result" for m in b2["messages"] for c in m["content"]), "anthropic: 第2次请求含 tool_result 块")
if oai:
    b = oai[0]["body"]
    chk(b["messages"][0]["role"] == "system", "openai: system 转为首条 system 消息")
    chk(any(t.get("type") == "function" and "parameters" in t.get("function", {}) for t in b.get("tools", [])), "openai: tools[].function.parameters")
if len(oai) >= 2:
    chk(any(m.get("role") == "tool" for m in oai[1]["body"]["messages"]), "openai: 第2次请求含 role=tool 消息")

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
  # Tab 0(protocol) → Tab 1(provider_name) → ... → Tab 5(确认)
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
  # 确认 Tab:默认选中 [确认],直接 Enter
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

echo "" | tee -a "$REPORT"
echo "==== 汇总: PASS=$PASS FAIL=$FAIL ====" | tee -a "$REPORT"
rm -rf /tmp/laew-e2e-root
[ "$FAIL" -eq 0 ]
