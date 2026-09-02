# laew TUI 交互优化与 `-f` 命令设计

> 版本：v0.1.2 ・ 日期：2026-09-02 ・ 状态：**已完成**

## 1. 背景与目标

`laew` 的 TUI 界面经历了两轮优化：

**v0.1.1（基于 rustyline）**：
1. **斜杠命令自动补全**：输入 `/` 后 Tab 补全
2. **新增 `-f` 参数**：从文件读取提示词执行（支持绝对/相对路径）
3. **路径自动补全**：TUI 内输入路径时支持目录/文件补全

**v0.1.2（基于 crossterm）**：
1. **修复 `//help` bug**：输入 `/` 再按 Tab 不再产生 `//help`
2. **下拉式补全列表**：输入 `/` 后实时显示命令列表，支持上下箭头导航
3. **灰色未确认状态**：未选中项显示灰色，选中项反白高亮
4. **自定义输入处理器**：基于 crossterm 实现，不再依赖 rustyline

## 2. 产品需求

### 2.1 斜杠命令补全交互

| 用户操作 | 系统行为 |
|----------|----------|
| 输入 `/` 后按 Tab | 显示所有可用斜杠命令列表 |
| 输入 `/pr` 后按 Tab | 唯一匹配 `/provider`，自动补全 |
| 输入 `/p` 后按 Tab | 唯一匹配 `/provider`，自动补全 |
| 输入 `/c` 后按 Tab | 唯一匹配 `/clear`，自动补全 |
| 输入 `/e` 后按 Tab | 唯一匹配 `/exit`，自动补全 |
| 输入 `/h` 后按 Tab | 唯一匹配 `/help`，自动补全 |
| Tab 多个匹配时 | rustyline 原生：显示列表，连续 Tab 循环选择 |
| 唯一匹配时 | Hinter 显示灰色后缀提示，按 → 或 End 接受 |
| 按 Enter | 执行当前输入的命令 |
| Ctrl-J（或终端 Shift+Enter） | 插入换行（多行输入） |

### 2.2 可用斜杠命令清单

```
/help (h, ?)          显示帮助信息
/exit (quit, q)       退出 TUI
/clear (c)            清空对话历史
/model                显示当前模型
/provider list (ls)   列出所有接入记录
/provider add         交互式新增接入记录
/provider use <id>    切换当前模型
/provider del <id>    删除接入记录
```

### 2.3 `-f` 文件参数

```
laew -f /path/to/prompt.md      绝对路径读取
laew -f ./prompt.md             相对路径读取（基于工作目录）
laew -f prompt.md               同上
```

行为：
- 读取文件内容，去除首尾空白后作为提示词执行
- 与 `-p` 互斥（同时指定报错）
- 文件不存在/为空/非 UTF-8 时给出友好错误提示

## 3. 技术设计

### 3.1 rustyline 14 Helper 架构

rustyline 14 通过 `Helper` trait 提供扩展能力：

```rust
pub trait Helper: Completer + Highlighter + Hinter + Validator {}
```

| Trait | 用途 | 本方案实现 |
|-------|------|-----------|
| `Completer` | Tab 补全，返回候选列表 | ✅ 实现 |
| `Hinter` | 行内灰色提示文本 | ✅ 实现 |
| `Highlighter` | 语法高亮 | ❌ 不使用 |
| `Validator` | 输入验证 | ❌ 不使用 |

### 3.2 TuiHelper 结构设计

```rust
/// TUI 辅助器：提供斜杠命令补全 + 路径补全 + 行内提示
struct TuiHelper;

impl Helper for TuiHelper {}
impl Completer for TuiHelper { ... }
impl Hinter for TuiHelper { ... }
impl Highlighter for TuiHelper { ... }  // 默认实现
impl Validator for TuiHelper { ... }    // 默认实现
```

### 3.3 补全逻辑流程

```
Completer::complete(line, pos)
    │
    ├─ line 以 '/' 开头？
    │   ├─ 是 → complete_slash_command(line, pos)
    │   │       ├─ 有匹配命令 → 返回候选列表
    │   │       └─ 无匹配 → 尝试路径补全
    │   └─ 否 → 检测是否为路径输入
    │           ├─ 是 → complete_path(line, pos)
    │           └─ 否 → 返回空（不补全普通提示词）
```

### 3.4 斜杠命令补全算法

```rust
fn complete_slash_command(line: &str, pos: usize) -> (usize, Vec<Pair>) {
    // 去掉开头的 '/'，获取用户已输入的文本
    let input = line[1..pos].trim();
    
    // 在命令列表中查找前缀匹配
    let matches: Vec<_> = SLASH_COMMANDS.iter()
        .filter(|cmd| cmd.starts_with(input))
        .collect();
    
    // 转换为 Pair（显示文本 + 替换文本）
    // 返回起始位置 1（跳过 '/'）和候选列表
}
```

### 3.5 路径补全算法

```rust
fn complete_path(line: &str, pos: usize) -> (usize, Vec<Pair>) {
    // 1. 展开家目录 '~' → $HOME
    // 2. 分离目录部分和文件名前缀
    // 3. 读取目录内容（std::fs::read_dir）
    // 4. 过滤匹配前缀的条目
    // 5. 目录条目追加 '/' 后缀
    // 6. 返回候选列表
}
```

### 3.6 行内提示（Hinter）

```rust
impl Hinter for TuiHelper {
    fn hint(&self, line: &str, pos: usize, ctx: &Context<'_>) -> Option<String> {
        if !line.starts_with('/') || line.len() < 2 {
            return None;
        }
        let input = line[1..].trim();
        // 查找唯一前缀匹配
        let matches: Vec<_> = SLASH_COMMANDS.iter()
            .filter(|cmd| cmd.starts_with(input) && *cmd != input)
            .collect();
        if matches.len() == 1 {
            Some(matches[0][input.len()..].to_string())
        } else {
            None
        }
    }
}
```

## 4. CLI 参数设计

### 4.1 参数定义

```rust
#[derive(Parser, Debug)]
struct Cli {
    #[arg(short = 'p', long = "prompt", conflicts_with = "file")]
    prompt: Option<String>,

    #[arg(short = 'f', long = "file", value_name = "PATH")]
    file: Option<PathBuf>,

    #[arg(long, default_value_t = 16, global = true)]
    max_iterations: usize,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}
```

### 4.2 执行优先级

```
1. provider 子命令 → cmd_provider()
2. -p 参数 → run_one_shot()
3. -f 参数 → run_from_file()
4. 无参数 → tui::run()
```

### 4.3 文件读取函数

```rust
async fn run_from_file(file_path: PathBuf, max_iterations: usize) -> Result<()> {
    // 1. 相对路径 → 基于工作目录解析
    // 2. std::fs::read_to_string 读取
    // 3. 空文件检查
    // 4. 调用 run_one_shot
}
```

## 5. 错误处理

| 场景 | 错误消息 |
|------|----------|
| 文件不存在 | `无法读取文件 '/path/to/file': No such file or directory` |
| 文件为空 | `文件 '/path/to/file' 内容为空` |
| 非 UTF-8 | `无法读取文件 '/path/to/file': invalid utf-8` |
| 权限不足 | `无法读取文件 '/path/to/file': Permission denied` |
| `-p` 与 `-f` 同时使用 | clap 自动报错：`argument '--prompt' cannot be used with '--file'` |

## 6. 文件变更

| 文件 | 变更 |
|------|------|
| `docs/TUI交互优化与-f命令设计.md` | 新增本文档 |
| `src/main.rs` | 添加 `-f` 参数、`run_from_file` 函数 |
| `src/tui/mod.rs` | 添加 `TuiHelper` 结构体、Completer/Hinter 实现、路径补全 |
| `CLAUDE.md` | 更新功能说明 |

## 7. 依赖

无需新增依赖。使用 rustyline 14 已有的 trait 和 API：
- `rustyline::completion::{Completer, Pair}`
- `rustyline::hint::Hinter`
- `rustyline::Helper`
- `rustyline::Context`

家目录展开使用 `std::env::var("HOME")`，无需额外 crate。

## 8. 验证方案

### 8.1 编译
```bash
./rebuild.sh
```

### 8.2 `-f` 功能
```bash
echo "你好" > /tmp/test.md
./laew -f /tmp/test.md          # 绝对路径
cd /tmp && ./laew -f test.md     # 相对路径
./laew -f /nonexistent.md        # 错误处理
```

### 8.3 TUI 补全
```bash
./laew
# 输入 "/" + Tab → 命令列表
# 输入 "/pr" + Tab → 补全为 "/provider"
# 输入 "/exit" + Enter → 退出
```
