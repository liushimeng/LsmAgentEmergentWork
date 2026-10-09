# TUI 输入处理常见陷阱与修复记录

## 1. 退格键在某些终端下不生效（2026-09-03 修复）

### 症状

在 TUI 交互模式下，输入文本后按退格键，每按一次退格不是原地编辑当前行，而是换行重新显示 `>>` 提示符 + 剩余文本：

```
>> /provider lis
>> /provider li
>> /provider l
>> /provider
>> /provide
...
```

### 根因

`InputHandler::read_line_inner()` 的退格处理只匹配了 `KeyCode::Backspace`：

```rust
KeyCode::Backspace => {
    // 删除光标前字符
    ...
}
KeyCode::Char(c) => {
    // 可打印字符：插入到光标位置
    buffer.insert(cursor, c);
    ...
}
```

某些终端环境下（Docker、SSH、TERM 类型不标准等），退格键发送的字符码未被 crossterm 映射为 `KeyCode::Backspace`，而是作为 `Char('\x7f')` (DEL) 或 `Char('\x08')` (BS) 传递。此时退格字符落入 `Char(c)` 分支被当作普通字符插入缓冲区，导致显示异常。

### 修复

在 `KeyCode::Backspace` 之后、`KeyCode::Char(c)` 之前，添加退格字符变体的兜底处理：

```rust
// src/tui/input.rs
KeyCode::Char('\x7f') | KeyCode::Char('\x08') => {
    // 兜底：处理退格字符变体（DEL=0x7f / BS=0x08）
    if cursor > 0 {
        cursor -= 1;
        buffer.remove(cursor);
        self.redraw_line(&mut stdout, prompt, &buffer, cursor, prompt_width)?;
        self.update_completion(...)?;
    }
}
```

### 教训

1. **crossterm 的 `KeyCode::Backspace` 不是万能的**：不同终端发送不同的退格字符码，crossterm 可能无法全部映射。
2. **match 分支顺序很重要**：`Char('\x7f')` / `Char('\x08')` 必须在通用 `Char(c)` 之前匹配，否则退格字符会被当作普通字符插入。
3. **终端兼容性需要显式处理**：不能假设所有终端都正确支持 raw mode 的所有方面。

### 验证

- 单元测试：`test_complete_backspace_scenario` 验证补全引擎在退格场景下的行为
- E2E 测试：`run_e2e.sh` Section 8 步骤10 使用 tmux `C-h` 发送退格验证原地编辑

### 相关文件

- `src/tui/input.rs` — `InputHandler::read_line_inner()` 退格处理
- `testReport/run_e2e.sh` — Section 8 步骤10 退格键测试
- `tmpPlan/01-退格键bug分析与修复方案.md` — 详细分析文档

---

## 2. tmux send-keys 退格键名称（2026-09-03 记录）

### 陷阱

在 tmux 自动化测试中，使用 `tmux send-keys Backspace` 不会发送退格控制字符，而是发送字面量文本 `"Backspace"`。

### 正确做法

```bash
# ❌ 错误：发送字面量文本 "Backspace"
tmux send-keys -t session Backspace

# ✅ 正确：发送 Ctrl+H（即 \x08，标准退格字符之一）
tmux send-keys -t session C-h
```

### 原因

tmux `send-keys` 的特殊键名列表中没有 `Backspace`。`Backspace` 被视为普通字符串（类似 `-l` 模式）。正确的退格键名是 `C-h`（Ctrl+H = `\x08`）。

### 其他常用 tmux 键名参考

| 键名 | 含义 |
|------|------|
| `C-h` | 退格（\x08） |
| `Enter` | 回车 |
| `Escape` | Esc |
| `Tab` | Tab |
| `Up` / `Down` / `Left` / `Right` | 方向键 |
| `C-c` | Ctrl+C |
| `C-d` | Ctrl+D |
| `BSpace` | 退格（部分 tmux 版本） |

---

## 3. 新增 TUI 子屏的断言锚点（通用指引）

### 规则

新增子屏时，`Screen::title()` 返回的字符串本身就是 tmux 断言锚点：

```rust
// src/tui/screen/xxx.rs
impl Screen for XxxScreen {
    fn title(&self) -> &str { "xxx title" }
    ...
}
```

在 `run_e2e.sh` 中：

```bash
tsubmit "/xxx"
texpect "xxx title" "tmux: 进入 Xxx 子屏"
```

### 注意事项

1. 断言文本必须与 `title()` 返回值完全一致
2. 使用 `texpect` 而非简单的 `grep`，因为子屏渲染需要时间
3. 子屏退出后验证回到主屏（检查 `>>` 提示符且无子屏边框字符）

## 4. 显示宽度按「字符数」算，CJK 场景盒子参差 + 进度行折行残影（2026-09-24 修复）

### 症状

启动横幅右边框不成一条直线，被截断的行还会顶穿边框：

```
║  项目说明: 未找到                                        ║   ← 60 列
║  工作区 : [通用] 非 git                                 ║    ← 59 列（短 1）
║  当前模型: [anthropic] liusm191-laew-model/liusm191-laew… ║  ← 61 列（溢出 1）
╚══════════════════════════════════════════════════════════╝
```

`/help` 同理（实测行宽 62/63/64/65 混合）；`[waiting]` 心跳行在 80 列终端上偶发半行残影。

### 根因（四条，互相独立）

1. **盒宽与行预算两套常量不自洽**：边框写死 58 内宽，每行内容按 `fit_display(v, 45/46)`
   独立填充。只有 `标签显示宽 + 预算 == 55` 的行才对齐 58，`工作区 :`(9)+45=54 就短 1 列。
2. **`truncate()` 省略号预算越界**：先截到 `≤max` 再 `out + "…"`，返回值宽度 = `max+1`，
   `fit_display` 的补空格算式 `max - w` 因此得 0 —— 任何被截断的行都比盒宽多 1 列。
3. **宽度按字符数而非显示列**：`truncate_chars(s, 40)` / `short_stage_label(.. MAX_CHARS=40)`
   放过 40 个汉字 = 80 列，加前缀必然折行。而 `[waiting]` 行靠「回车回列 0 + `ESC[K`」
   **原地重写**，一旦折行，下一次回车回到的是物理行首而非屏幕行首 → 残影。
4. **盒宽与终端无关**：58 列内宽固定，100+ 列终端上 `当前模型`（含 endpoint）、长路径
   照样被砍尾 —— 用户主观感受即「信息显示不全」。

### 修复

显示宽度度量与盒线排版收敛到一个真源 `src/tui/textfit.rs`：

| 不变量 | 保证方式 |
| ------ | -------- |
| 截断结果宽度 `≤ max` | `clip` / `clip_mid`：省略号**计入预算**（不再追加） |
| 盒线严格等宽 | `InfoBox::render()`：内宽 = `min(内容自然宽, 终端可用宽)`，逐行 `pad` 到同一内宽 |
| 长信息不砍尾 | `RowKind::Kv` 超长**折行续排**（续行缩进到值列）；路径/URL 用 `KvKeepTail` 中间省略保尾 |
| 进度行不折行 | `stage_line_text()` / `short_stage_label()` 按显示列收敛，预留前缀+计时开销 |
| 三处度量不分叉 | `input::char_width`/`display_width` 转发 `textfit`，横幅与输入行同一套列数 |

新增度宽歧义开关：`LAEW_AMBIGUOUS_WIDE=1` 让 `✓ · … → ║` 等 East Asian Ambiguous 字符按
2 列参与**计算**（不改渲染），供把盒线渲染成双宽的老终端（部分 Windows conhost 中文字体 /
xterm `-ctwidth`）校正对齐；默认关闭 = 与修复前逐字节一致。

### 校验

- 单测：`textfit::infobox_各行严格等宽_多种终端宽`（40~200 列全等宽且不溢出）、
  `banner::窄终端折行而非砍尾_且一个字符都不丢`（折行后拼回原文零损失）。
- e2e：`run_e2e.sh` §7 抓真机横幅 `╔…╚` 区块，python3 按 `east_asian_width` 复算行宽，
  断言集合大小 == 1（`command -v python3` 缺失时 SKIP）。

### 注意事项

1. **新增任何盒线/信息块一律走 `textfit::InfoBox`**，不要手写 `"═".repeat(58)` 或逐行调空格。
2. 度量只用 `textfit::width`；按 `chars().count()` 估宽在 CJK 下必然低估一倍。
3. 原地重写类行（`\r` + `ESC[K`）必须先保证「整行不超终端宽」，否则 ANSI 纪律失效。

---

## N. 终端消失后进程空转不退、`kill` 杀不掉（2026-10-09 第 136 轮修复）

### 症状

macOS 活动监视器里堆积十几个 `laew` 进程，每个 CPU 时间 600+ 分钟、%CPU 60~70，
合计吃掉约 9 个核心；`lsof` 显示 `/dev/ttys0xx` **只有 laew 自己持有**（原始终端
早已不存在）。`kill <pid>` 无效，只有 `kill -9` 能杀。

### 根因（两层，叠加才成灾）

**第一层 · crossterm 0.27 在「fd 可读却读不到字节」时内部死转。**
`crossterm-0.27.0/src/event/source/unix/mio.rs:95-120`：

```rust
TTY_TOKEN => {
    loop {                                   // ★ 无超时检查、无 EOF 检查
        match self.tty_fd.read(&mut self.tty_buffer, TTY_BUFFER_SIZE) {
            Ok(read_count) => { if read_count > 0 { self.parser.advance(..); } }
            // ↑ Ok(0)(EOF)什么都不做,直接落回循环
            Err(e) => {
                if e.kind() == WouldBlock { break; }
                else if e.kind() == Interrupted { continue; }
                // ★ 其它 Err(EIO 等)同样落回循环
            }
        };
        if let Some(event) = self.parser.next() { return Ok(Some(event)); }
    }
}
```

pty master 关闭（终端窗口/标签被关、SSH 断线）后，slave 上的 `read()` **立即返回
0 字节并永远如此**。`event::poll(timeout)` 与 `event::read()` 在这个状态下**都会
永久 hang** —— 超时检查在外层 `mio.rs:154`，内层根本走不到，调用方**无法从外部察觉**，
表现为 100% CPU 空转。

**第二层 · SIGHUP / SIGTERM 被 handler 吞掉，而主线程回不到检查点。**
`tui/mod.rs` 主循环是「先查 `is_triggered()`，再 `read_line()`」，检查点在阻塞之前；
`shutdown.rs` 的 `trigger()` 只写标志位 + cancel + notify，**不导致任何退出**
（`grep -rn "Hangup" src/ | grep -v shutdown.rs` 零命中）。而注册 handler 的副作用是
**内核不再按默认动作终止进程**。于是终端关闭 → SIGHUP 被吞 → 主线程死转 → 永不退出；
`kill` → SIGTERM 被吞 → 同样永不退出。

> 原 `tui/mod.rs` 注释「即使 read_line 在阻塞等用户输入，也能立即 break」是错的。

### 两种 pty 终态（决定修复要分两条腿）

| 场景 | `poll(POLLHUP)` | `read()` | 行为 |
| --- | --- | --- | --- |
| 会话首进程已死（孤儿 pty，现场那批） | **立即触发**（实测 0.000s） | 0 字节 EOF | crossterm **死转** |
| 会话首进程存活 | 不触发 | EAGAIN | 正常阻塞，但 SIGHUP 被吞 → 不退 |

`POLLHUP` 无条件上报，只订阅它就够（不带 `POLLIN` 可避免「有按键待读」被误判）。

### 修复

1. **终端守卫看门狗**（新增 `src/tui/term_guard.rs`，`main.rs` 安装）：独立
   `std::thread`，1s 一轮 —— ① `stdin_hangup()`（POLLHUP）命中即清理退出；
   ② shutdown 已触发且超 10s 宽限 → 强制退出（让 `kill` 真正有效；实测 Ctrl-C
   取消到退出约 2s，10s 是 5 倍余量）。`isatty(0)` 门控，非 TTY 一律跳过。
2. **收口路径的两个坑**（比根因更隐蔽）：
   - `finish()` 一开始调 `shutdown::terminal_restore_sync()`，它抢 **Rust stdio 全局
     互斥锁**，主线程卡死时永不释放 → 守卫线程一起卡死 → 日志明明打了「强制退出」，
     进程却还活着。改用 `libc::write(2)` 直写 fd 1/2 的 `force_terminal_restore()`。
   - 再加保险丝线程：3s 后无条件 `process::exit`，任何清理步骤卡住都不会退化成
     第二个杀不掉的进程。
3. **堵住其余读事件入口**：同一死转在工程里有 4 个调用点，全部补「进 `event::read()`
   前先做 0 超时挂断探测」——`read_line_inner` 主循环、快速输入排空内循环、
   `drain_paste_burst`、**`engine.rs::read_key`（`/provider *` 子屏，与主屏同源同病）**。
4. **相邻缺陷**：`tui/mod.rs` 两条提前 `return Err` 路径跳过 `terminal_restore()`
   （关掉后终端滚动区/底部面板残留）；TUI 所有退出路径都不调 `BrowserManager::shutdown()`
   （纯靠 `#[cfg(unix)]` 的 atexit，**Windows 上既无 atexit 也无 Drop**，Chrome 留在后台）；
   `macos_legacy` 的 `Command::new("open").spawn()` 返回值被丢弃（每次调用泄漏一个 zombie）；
   `human_ui` 的 payload 临时文件用「await 之后 remove_file」清理（弹窗取消会 drop
   整个 future，await 之后不执行 → 每次漏一个 `/tmp/*.json`）。

### 实测验证

| 场景 | 修复前 | 修复后 |
| --- | --- | --- |
| `kill -TERM` 后 25s | 仍在运行，只能 SIGKILL | **15s 退出** |
| `kill -HUP` / `kill -INT` | 仍在运行 | **15s 退出** |
| 关闭终端 master（孤儿 pty） | 2s 退出 | 2s 退出（无回归） |
| 健康 TUI 连续 45s | 存活 | **存活，零误杀** |
| e2e 跑完后残留进程 | — | **0 个** |

单测 7 个，含死锁回归 `force_terminal_restore_does_not_block_on_stdio_lock`
（另一线程霸占 stdout 锁 3s，要求 `force_terminal_restore()` 1s 内返回）。

### 平台差异

- **macOS/Linux**：`POLLHUP` 语义完整，看门狗全量生效；
- **Windows**：无 pty，`stdin_hangup()` 恒 `false`，但**信号宽限兜底仍生效**
  （`kill` 不会失效），且本轮补的显式 `BrowserManager::shutdown()` 正好填上
  Windows 侧浏览器回收的空缺。

### 仍未覆盖的边界（如实记录）

若 macOS 既不投递信号、也不置 `POLLHUP`（实测：pty 会话首进程存活时关闭 master
就是这种状态），进程内部**没有任何可观测变化**，无法与「用户正坐在空闲提示符前」
区分。这类状态本轮无法覆盖；但**「kill 杀不掉」这一类已被彻底解决** —— 无论进程
处于哪种状态，`kill` 后最多 10s 必退。
