# 23 — macOS Chrome 首启弹窗与 Dock 图标根治

> 第 171 轮(2026-10-14)。解决 `MCP_Web_Use` 在 macOS 上
> ① 经常弹「欢迎使用 Google Chrome」对话框;② Dock 右侧总是新增 Chrome 图标。

## 1. 根因

### 1.1 chromiumoxide 参数「双横杠」坑(欢迎框主因)

`chromiumoxide` 的 `BrowserConfigBuilder::arg(s)` 走 `Arg::from(&str)`,把整串当 key,
渲染时统一补 `--` 前缀(`browser/argument.rs::into_iter` → `format!("--{}", key)`)。
laew 传 `.arg("--no-first-run")` 被渲染成 **`----no-first-run`**,Chrome 当未知开关
**静默忽略**。

实测本机残留的 laew 拉起 Chrome 的命令行(所有自定义开关全带 `----`):

```
Google Chrome --disable-extensions ----disable-ipc-flooding-protection ----disable-sync
  ----no-default-browser-check ----disable-hang-monitor ----no-first-run ...
  --user-data-dir=/var/folders/.../laew_browser_6ce0031f ...
```

后果:`--no-first-run` / `--no-default-browser-check` 失效 → 每次冷启动弹首启对话框;
`--ignore-certificate-errors` / `--disable-features=...` / `--disable-blink-features=...`
等反检测与证书开关一并失效;缺 `--disable-background-mode` → Chrome 后台模式开启。

> 对照实验(本机):新 profile 下不加 `--no-first-run` → 弹「登录 Chrome / 欢迎使用
> Google Chrome」;加 `--no-first-run --no-default-browser-check` → 仅「新标签页」。
> 证明旗标有效,问题在参数被渲染成 `----xxx`。

### 1.2 Dock 图标:新实例 + 后台常驻

- laew 每次以独立 `--user-data-dir` 另起 Chrome **实例**,macOS 为每个运行中的 GUI 应用
  在 Dock 显示图标;`com.apple.dock` 的 `recent-apps` 实测累积多条 `Google Chrome`
  记录(GUID 连号,逐次新增),即「底部右侧总是出现新的 Chrome 图标」。
- Chrome **后台模式**默认开启时,关闭窗口后进程仍驻留,Dock 图标不消失,表现为
  「自己安装图标到系统的底部 Bar 中」。

> macOS 上「可见窗口的应用必有 Dock 图标」是系统行为,无法在不改系统 Chrome 的前提下
> 让运行中的 Chrome 完全无图标(跨进程 `TransformProcessType` 在 macOS 27 返回 `-600`,
> `NSRunningApplication.setActivationPolicy` 不存在,均实测排除)。可治理的是**图标驻留/累积**
> 与**首启弹窗**。

## 2. 方案

### 2.1 裸名参数(根治欢迎框)

`custom_launch_flags(headless, geo_prompt) -> Vec<String>` 集中返回**裸名**开关,
由 chromiumoxide 统一补前缀。launch 路径改为遍历该集合。单测锁死「开关必须裸名」。

### 2.2 profile 预置(双保险)

`seed_chrome_profile(dir)` 在 launch 前写入:

- `First Run` 哨兵(存在即非首启);
- `Local State`:`background_mode.enabled=false`、`exited_cleanly=true`、`reporting_enabled=false`;
- `Default/Preferences`:`has_seen_welcome_page=true`、`skip_welcome_page=true`、
  `first_run_tabs=[]`、`session.restore_on_startup=4`(新标签页)。

### 2.3 auto_relaunch 路径同步治理

`browser_reuse::relaunch_args` / `relaunch_command_string` 此前完全无首启抑制旗标,
是欢迎框另一来源。补齐治理旗标,并在复制登录态后调用 `seed_chrome_profile`。

### 2.4 Dock 图标驻留治理

`--disable-background-mode` + `--no-service-autorun`:Chrome 随窗口关闭完全退出,
不再驻留后台、不再在 Dock 保留图标。复用系统已安装 Chrome,不安装新浏览器。

## 3. 改动

| 文件 | 改动 |
| --- | --- |
| `src/agent/browser.rs` | 新增 `custom_launch_flags` / `seed_chrome_profile`;launch 路径改裸名参数 + 预置 profile;补单测 |
| `src/agent/browser_reuse.rs` | `relaunch_args` / `relaunch_command_string` 补治理旗标;`relaunch_and_wait` 预置 profile;更新单测 |

## 4. 验证

- `cargo test --lib` 全绿(含新增裸名/预置单测)。
- 真机冒烟:launch 后 `ps` 确认参数为 `--no-first-run`(单横杠);CDP 仅「新标签页」,
  无「欢迎使用 Google Chrome」窗口。
