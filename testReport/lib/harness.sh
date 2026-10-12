# harness.sh: e2e 前奏 —— 产物路径/并发锁/mock 端口探测/计时与断言 helper/沙箱引导。
# 由 run_e2e.sh 在小节调度前 source;小节可自由使用这里定义的 helper。
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

# ---- 产物清理 ----
# 单次全量跑会产生 ~25MB mock 抓包 jsonl + ~1MB 报告;不清理的话 testReport/
# 几天就膨胀到几百 MB,后续「读一下 testReport/*」排查故障的人(和 Agent)会被
# 淹没在历史产物里。策略:开工前只留最近 KEEP_RUNS 轮;收尾时全部通过则删本轮
# 抓包(报告保留),有失败则保留并在汇总里打出路径。
KEEP_RUNS="${LAEW_E2E_KEEP_RUNS:-3}"
prune_old_artifacts() {
  # 旧报告:按 mtime 倒序,保留最近 KEEP_RUNS 份
  ls -t testReport/e2e-*.txt 2>/dev/null | tail -n +$((KEEP_RUNS + 1)) | while read -r f; do
    rm -f "$f"
  done
  # 旧抓包:mock_requests-*.jsonl
  ls -t testReport/mock_requests-*.jsonl 2>/dev/null | tail -n +$((KEEP_RUNS + 1)) | while read -r f; do
    rm -f "$f"
  done
  # tmux 运行残留 + 脚本备份
  rm -f testReport/tmux-laew_e2e_*.log testReport/.e2e-filtered-*.sh
}
prune_old_artifacts

# ---- mock 端口就绪探测 ----
# 原先每个 mock 服务起来后固定 sleep 0.6(全量 25 处 ≈ 15s 纯等待),既慢又不
# 稳(机器繁忙时 0.6s 不够、且端口占用时报错被 sleep 掩盖)。改为轮询探测:
# 端口可连接即返回,典型 <30ms。
port_open() { (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null && exec 3<&- 3>&-; }
wait_port() {  # $1=端口 $2=mock 进程 pid(可选,用于进程已死时提前退出)
  local port="$1" pid="${2:-}" i
  for ((i = 0; i < 300; i++)); do
    if port_open "$port"; then return 0; fi
    [ -n "$pid" ] && ! kill -0 "$pid" 2>/dev/null && return 1
    sleep 0.02
  done
  return 1
}
start_mock() {  # $1=端口 $2=pid变量名 其余=mock_llm_server.py 参数
  local port="$1" pidvar="$2"; shift 2
  python3 scripts/mock_llm_server.py "$@" &>/dev/null &
  local pid=$!
  if ! wait_port "$port" "$pid"; then
    echo "  [WARN] mock 端口 $port 未在 6s 内就绪(继续,后续断言会失败)" | tee -a "$REPORT"
  fi
  printf -v "$pidvar" '%s' "$pid" 2>/dev/null || eval "$pidvar=$pid"
}

# ---- 逐小节耗时 ----
# section() 顺手记账,收尾打印「最慢 Top10」+ 总耗时;--timing 额外打全表。
# 用于回答「改哪个模块该跑哪几节」——先看表,再用 --only 定向跑。
E2E_T0=$SECONDS
SEC_NAMES=(); SEC_ELAPSED=(); SEC_IDX=0; SEC_T0=$SECONDS
section() {
  local now=$SECONDS
  if [ "$SEC_IDX" -gt 0 ]; then
    SEC_ELAPSED[$((SEC_IDX - 1))]=$(( now - SEC_T0 ))
  fi
  SEC_NAMES[$SEC_IDX]="$1"
  SEC_T0=$now
  SEC_IDX=$((SEC_IDX + 1))
  echo "" | tee -a "$REPORT"; echo "==== $1 ====" | tee -a "$REPORT"
}
close_last_section() {
  [ "$SEC_IDX" -gt 0 ] || return 0
  SEC_ELAPSED[$((SEC_IDX - 1))]=$(( SECONDS - SEC_T0 ))
}
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

# 准备:独立数据库(避免污染真实配置) —— 注意:小节分隔注释统一用 `# --- ` 前缀,
# 本行刻意不带,以便被 --only/--skip 过滤时始终留在前奏里(前奏含 run/check/
# section/wait_port 等全部 helper 定义,丢掉就没有可执行骨架了)。
# 2026-09-10 第二十八轮修复:rm -f 对目录无效,残留旧 DB 会让 §2「未配置模型」
# 与 §3 provider add(UNIQUE 冲突)级联失败;必须 rm -rf 保证干净起点。
rm -rf /tmp/laew-e2e-root; mkdir -p /tmp/laew-e2e-root
cp laew /tmp/laew-e2e-root/laew
LAEW=/tmp/laew-e2e-root/laew   # 根目录=/tmp/laew-e2e-root → db 也在这里
