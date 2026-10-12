# --- 8. TUI 子屏自动化(tmux control-mode,真 PTY 渲染) ---
# 由 testReport/run_e2e.sh 自动 source;小节 id=8 —— 改这里不要动入口脚本。
# 详见 docs/TUI自动化测试/01-设计与解决方案.md
section "8. TUI 子屏自动化(tmux control-mode)"
if ! command -v tmux >/dev/null 2>&1; then
  echo "  [SKIP] 系统未安装 tmux,跳过子屏自动化测试" | tee -a "$REPORT"
else
  TSESS="laew_e2e_$$_${RANDOM}"
  TMUX_LOG="testReport/tmux-${TSESS}.log"
  : > "$TMUX_LOG"
  # 任一路径退出都杀会话,避免污染:小节收尾显式 kill,进程级 EXIT trap 兜底。
  # (拆分前这里是顶层 RETURN trap;独立成文件被 source 后它会在小节开头
  #  即触发并杀掉会话,故改为等价的显式收尾。)

  # ----- tmux helpers -----
  tnew() {  # 创建后台会话并启动 laew,固定 100x30
    # 2026-09-10 第 25 轮:env 显式内联 —— 脚本 shell 的 export 对常驻 tmux server
    # 的会话环境不生效(server 只继承启动时环境 + update-environment 白名单),
    # 隔离库中的 127.0.0.1 provider 会在 TUI bootstrap 时被 SSRF 守卫拦截致秒退,
    # 全部 tmux 用例空捕获(实测 32 FAIL)。
    tmux new-session -d -s "$TSESS" -x 100 -y 30 "env LAEW_ALLOW_PRIVATE_ENDPOINT=1 $LAEW" 2>>"$TMUX_LOG"
    # 等待 bootstrap(banner 是首屏内容,出现即说明已就绪)。轮询而非固定 sleep:
    # 机器繁忙时 0.5s 不够会漏掉横幅断言,空闲时又白等 —— 轮询两头都快。
    local i
    for ((i = 0; i < 100; i++)); do
      tscreen | grep -q "程序目录" && return 0
      sleep 0.05
    done
    return 0
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
  texpect "程序目录" "tmux: 横幅显示程序目录(第 152 轮:根目录改名)"
  texpect "工作目录" "tmux: 横幅显示工作目录"
  texpect "产物目录" "tmux: 横幅显示产物目录(第 152 轮)"
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
  # 清理:Ctrl-U 清空当前输入(空输入 Ctrl-C 会按当前 TUI 语义退出)
  tkey C-u
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

  # 14d) 多行粘贴保真(2026-09-24 修 R5):3 行 Markdown 提示词过去被判「小粘贴」→
  #      换行全替换成空格,屏幕上只剩一行、送进模型也丢了列表结构。现在 ≥2 行即转
  #      marker 保真,并立刻在滚动区回显原文预览。
  PASTE3=$(printf '### 标题行\n1. 步骤一\n2. 步骤二')
  tsend "$(printf '\x1b[200~%s\x1b[201~' "$PASTE3")"
  sleep 0.6
  # 断言不钉死粘贴编号(#N 会话内单调递增,前序用例已占号),只锁行数保真
  texpect "+3 行]" "tmux: 多行粘贴转 marker(换行不再被压成空格)"
  texpect "原文已保留(3 行" "tmux: 粘贴后即时回显原文预览"
  tkey C-u; sleep 0.3

  # 14e) 粘贴内容「只显示一遍」(2026-10-08 第 131 轮):粘贴瞬间已回显前 4 行预览,
  #      提交时若再全量回显一遍,8 行提示词在屏幕上连着出现两次,观感像「粘了两遍」。
  #      现在提交只回显输入行 marker 形态 + 一行「已按原文完整发送 N 行 / M 字」。
  #      mock LLM 不会真跑(该轮只验输入层回显),故用 /help 走一次提交即可。
  PASTE_ONCE=$(printf 'ZZ标记第一行\nZZ标记第二行\nZZ标记第三行')
  tsend "$(printf '\x1b[200~%s\x1b[201~' "$PASTE_ONCE")"
  sleep 0.6
  texpect "原文已保留(3 行" "tmux(131): 粘贴预览出现"
  tkey Enter
  sleep 0.8
  texpect "已按原文完整发送 3 行" "tmux(131): 提交时标注完整发送行数"
  # 关键断言:预览之外,提交回显不得再出现原文 —— 整个面板里每行原文只应出现 1 次
  ONCE_OK=1
  for i in 1 2 3; do
    n=$(tscreen | grep -F -c -- "ZZ标记第${i}行" || true)
    [ "${n:-0}" -le 1 ] || ONCE_OK=0
  done
  [ "$ONCE_OK" -eq 1 ] \
    && check 0 "tmux(131): 粘贴原文全程只显示一遍(提交不重复回显)" \
    || { check 1 "tmux(131): 粘贴原文全程只显示一遍(提交不重复回显)"; tscreen | sed 's/^/    | /'; }
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

  tmux kill-session -t "$TSESS" 2>/dev/null || true
  rm -f "$TMUX_LOG"
fi
