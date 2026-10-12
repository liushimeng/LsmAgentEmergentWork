# --- 5b. 项目说明文件发现与首次注入(工作目录五级链) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=5b —— 改这里不要动入口脚本。
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
