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
