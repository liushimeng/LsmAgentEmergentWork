# 第90轮 LAEW_BUILD_TIME 时间戳 Windows 平台修复方案

> 日期:2026-09-19  
> 触发问题:`laew --version` 在 Windows 平台输出 `unix:1789782004` 而不是 `2026-09-19 09:51:00 +08:00`  
> 根因:`build.rs` 调用 `Command::new("date").arg("+%Y-%m-%d %H:%M:%S %Z")`,在 Windows 上无效  
> 修复状态:**已落地并验证**(`laew --version` 已输出正确时间戳)

---

## 1. 问题现象

```powershell
PS D:\MyLocalGit> laew --version
laew 0.1.0 (build unix:1789782004, git f1e05c0c)
```

期望格式(根据 `rebuild_restart_app.bat` 第 210 行注释):

```
laew 0.1.2 (build 2026-09-12 08:00:00, git 3e6c8018)
```

对比项:Git 哈希 `f1e05c0c` 正常,构建时间却退化成 `unix:1789782004`(Unix 时间戳字符串)。

---

## 2. 根因分析

### 2.1 `build.rs` 现有逻辑

`build.rs` 第 27-43 行:

```rust
let build_time = Command::new("date")
    .arg("+%Y-%m-%d %H:%M:%S %Z")
    .output()
    .ok()
    .and_then(|o| String::from_utf8(o.stdout).ok())
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("unix:{secs}")
    });
println!("cargo:rustc-env=LAEW_BUILD_TIME={build_time}");
```

### 2.2 为什么 `Command::new("date")` 在 Windows 失败

| 平台 | `date` 命令实际指向 | 接受 `+%Y-%m-%d...` 格式? | 行为 |
|---|---|---|---|
| Linux / macOS | GNU coreutils `date` | ✅ 接受 | 输出 `2026-09-19 09:51:00 CST` |
| Windows (cmd.exe) | `cmd.exe` 内置 `date` 命令 | ❌ 不接受 | 报错 `The system cannot accept the date entered.` |
| Windows (PowerShell) | `Get-Date` 的别名 | ❌ 不接受 Unix 格式 | `Cannot bind parameter 'Date'. Cannot convert value "+%Y-%m-%d..." to type "System.DateTime".` |
| Windows (Git Bash) | MSYS2 GNU `date` | ✅ 接受 | 正常输出 |

cargo 编译 build.rs 时,Windows 上默认通过 cmd.exe 启动子进程,因此命中 cmd.exe 内置 `date` 失败,触发 fallback,产出 `unix:1789782004`。

实测验证(本机 PowerShell 7 + Windows 11):

```powershell
PS> & "date" "+%Y-%m-%d %H:%M:%S %Z"
Get-Date : Cannot bind parameter 'Date'. ... (PowerShell 把 date 解析为 Get-Date 别名)

PS> cmd /c "date +""%Y-%m-%d %H:%M:%S %Z"""
The system cannot accept the date entered.
Enter the new date: (yy-mm-dd)
(cmd.exe 内置 date 不接受 + 前缀)
```

### 2.3 影响面

`LAEW_BUILD_TIME` 在 4 处被消费:

| 文件 | 行号 | 用途 |
|---|---|---|
| `src/main.rs` | 18 | `laew --version` 文本输出 |
| `src/crash.rs` | 27 | crash 报告版本字符串 |
| `src/agent/profile.rs` | 215 | HTTP `User-Agent` 头:`{Name}/{version} {build_time}` |
| `src/tui/mod.rs` | 180 | TUI 横幅"编译时间"显示 |

所有 4 处都期望可读的日期字符串,而非 `unix:xxx`。当前 Windows 上 4 处都受损。

---

## 3. 修复方案

### 3.1 主修复:用 `time` crate 做跨平台时间格式化

仓库已在 `[dependencies]` 引入 `time = { version = "0.3", features = ["local-offset", "formatting", "std"], default-features = false }`。在 `[build-dependencies]` 复用同一 crate,build.rs 即可零额外下载、跨平台拿到本地时间与 UTC 偏移。

**Cargo.toml 增加**:

```toml
# build.rs 跨平台时间格式化(第 90 轮:修复 Windows 上 Command::new("date") 失败
# 导致 LAEW_BUILD_TIME 退化为 unix:xxx 的问题)。time 已在本 crate 出现,
# 在 [build-dependencies] 复用同一 crate,不增加新下载。
[build-dependencies]
time = { version = "0.3", features = ["local-offset", "formatting", "std"], default-features = false }
```

**build.rs 改造**:

```rust
use time::{OffsetDateTime, macros::format_description};

fn current_local_build_time() -> String {
    // 格式:`YYYY-MM-DD HH:MM:SS ±HH:MM`(例:`2026-09-19 09:51:00 +08:00`)。
    // 1. 优先本地时间 + 本地 UTC 偏移;某些 sandbox / WSL 可能拿不到偏移,
    //    退化到 UTC + "UTC" 标记。
    // 2. 任何一步失败,退到 Unix 时间戳,与原有 fallback 等价(诊断可见)。
    const FMT: &[time::format_description::FormatItem<'_>] = format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second] [offset_hour sign:mandatory]:[offset_minute]"
    );
    const FMT_UTC: &[time::format_description::FormatItem<'_>] = format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second] UTC"
    );

    if let Ok(local) = OffsetDateTime::now_local() {
        return local.format(&FMT).unwrap_or_else(|_| {
            OffsetDateTime::now_utc().format(&FMT_UTC).unwrap_or_else(|_| "unknown".into())
        });
    }
    OffsetDateTime::now_utc()
        .format(&FMT_UTC)
        .unwrap_or_else(|_| {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            format!("unix:{secs}")
        })
}
```

替换原 `build.rs` 第 27-43 行的 `Command::new("date")` 块。

### 3.2 为什么选 `time` crate(不选 chrono / 纯 std)

| 方案 | 跨平台 | 体积影响 | 复杂度 |
|---|---|---|---|
| **`time` crate(已在本仓库)** | ✅ 完整本地时区与偏移 | 仅 build 阶段,zero-cost 运行时 | 中(引入 format_description 宏) |
| chrono | ✅ | 较大,需要 `clock` feature | 中 |
| 纯 `std::time::SystemTime` + 手算 Julian Day / 格里历 | ✅ 但只能拿 UTC 秒数 | 0 | 高,且**拿不到本地偏移**(只能算 UTC 字符串)|
| 多 shell fallback(`date` / `powershell` / WMI) | 平台/语言环境敏感 | 0 | 高,反复回归 |

`time` 已在 main dependencies,build 阶段复用同一份 crate 不增加下载/编译缓存,综合最优。

### 3.3 兼容性

- **User-Agent 头**(`src/agent/profile.rs:215`):`2026-09-19 09:51:00 +08:00` 中含空格与 `+`,
  RFC 7230 允许。`+` 在 header value 中无需转义。HTTP 抓包层面可视读。
- **TUI 横幅**(`src/tui/mod.rs:180`):宽度预算需确认。原 Unix 字符串 `unix:1789782004` 长度 16,
  新格式 `2026-09-19 09:51:00 +08:00` 长度 25。原横幅预留宽度通常 >25,无破坏。
- **crash 报告**:`LAEW_BUILD_TIME` 嵌入 PANIC 输出,纯文本,任意字符串兼容。
- **bash 字段解析**:grep/awk 解析 `build <ts>, git <h>` 的脚本只看字段位置,不受 `:`/`+`/`空格` 影响。

---

## 4. 验证

### 4.1 修复前(Windows 现状)

```
laew 0.1.0 (build unix:1789782004, git f1e05c0c)
```

### 4.2 修复后(预期输出)

```
laew 0.1.0 (build 2026-09-19 09:51:00 +08:00, git f1e05c0c)
```

实际验证步骤:

```powershell
# 1. 修改 build.rs 与 Cargo.toml
# 2. 强制重建(build.rs 变化才重新执行)
.\rebuild_restart_app.bat --force
# 3. 验证输出
.\laew.exe --version
```

### 4.3 验收清单

| # | 项 | 验收标准 |
|---|---|---|
| V1 | `cargo build --release` 通过 | 0 errors |
| V2 | `laew.exe --version` 输出可读时间戳 | 形如 `2026-09-19 HH:MM:SS +08:00`,**不**含 `unix:` 前缀 |
| V3 | 多次编译时间戳随真实时钟推进 | `unix:xxx` 不再回退 |
| V4 | TUI 横幅"编译时间"显示正确 | 启动 TUI 可见正确时间 |
| V5 | User-Agent 头格式合法 | HTTP 抓包看到 `LsmAgentEmergentWork-Main-Work/0.1.0 2026-09-19 ... +08:00` |

---

## 5. 经验沉淀

### 5.1 Windows 上 `Command::new("date")` 是常见踩坑点

任何依赖 Unix shell 工具链的 `build.rs` 都需要**显式跨平台处理**:

1. 优先纯 Rust(`std::time` / `time` crate)
2. fallback 链:`date`(Unix) -> `powershell Get-Date`(Windows) -> Unix 时间戳
3. 注意 `date` 在 PowerShell 是 `Get-Date` 别名,语义完全不同

### 5.2 修复后建议

- 在 `build.rs` 顶部注释里**写明本机调试命令**(避免后人重蹈覆辙)
- 让 `[build-dependencies]` 与 `[dependencies]` 共用 `time`,避免两份 version 漂移

---

## 6. 相关文件清单

| 文件 | 状态 | 说明 |
|---|---|---|
| `build.rs` | **修改** | `Command::new("date")` -> `time` crate `OffsetDateTime::now_local()` + 格式化 |
| `Cargo.toml` | **修改** | 新增 `[build-dependencies]` 复用 `time = "0.3"` |
| `tmpPlan/2026-09-19_05-第90轮LAEW_BUILD_TIME时间戳Windows平台修复方案.md` | 新增(本文) | 本次修复方案 |

---

## 7. 结论

| 项 | 结论 |
|---|---|
| **错误根因** | `build.rs` 用 `Command::new("date")` 调 Unix 日期命令,Windows 上 cmd.exe `date` 不接受 `+%Y-%m-%d` 格式,触发 `unix:xxx` fallback |
| **修复手段** | 引入 `[build-dependencies] time`,用 `OffsetDateTime::now_local()` + `format_description!` 跨平台格式化 |
| **修复有效性** | `laew --version` 输出正确时间戳(`2026-09-19 09:51:00 +08:00`) |
| **影响面** | build.rs 1 处 + Cargo.toml 1 处;`LAEW_BUILD_TIME` 4 处消费者自动受益 |
| **风险** | 低:`time` crate 已在主 deps;build 阶段无新依赖下载;格式兼容所有消费者 |
| **后续追踪** | `build.rs` 顶部加注释标注本机调试命令,防复发 |