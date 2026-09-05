# AtomCode 第三轮边缘模块深度分析

> 分析日期: 2026-09-05
> 分析范围: 前三档文档未覆盖的目录/模块 + 被遗漏的 crate
> 已有文档: `atomcode-源码调研.md` / `atomcode-深度分析.md` / `atomcode-核心机制深度分析.md` / `atomcode-第二轮深度分析.md`
> 已覆盖主题: 14 crates L0/L1/L2 分层、cargo feature gating、kernel trait、AgentCommand/AgentEvent 协议、MCP 7 子模块、Skill、CodeIntel 七件套、tracing 遥测、多熔断计数器

---

## 一、未覆盖目录结构总览

```
atomcode/
  webui/              ← Preact SPA, daemon 内嵌式 Web UI
  extensions/
    vscode/           ← VSCode 扩展(React webview + esbuild)
    jetbrains/        ← JetBrains 插件(Kotlin + IntelliJ Platform)
  evals/
    deepseek-v4-flash/ ← 双候选配对评估框架(Python stdlib)
  docker/             ← Daemon/TUI 两套 Dockerfile + docker-compose
  scripts/            ← 构建/发布/签名/测试/安装脚本
  site/               ← 静态文档站(双语 + 搜索索引)
  packages/
    npm/              ← npm 分平台 wrapper 包
    homebrew/         ← Homebrew 打包脚本
  examples/
    hooks/            ← Hook 配置示例(toml + shell)
  .github/            ← GitHub CI/CD 配置
```

**判断逻辑**: 以下按「对 laew 借鉴价值」排序深挖,纯配置/脚手架简写。

---

## 二、逐目录深挖

### 2.1 webui/ — 内嵌式 Web UI (高价值,深挖)

#### 2.1.1 架构选型

技术栈: **Preact + Tailwind CSS + Vite**。选 Preact 而非 React 的核心考量是打包体积(Preact 3KB gzip vs React 45KB),适合嵌入二进制。

```
webui/src/
  api.ts              ← 完整 REST/SSE API 客户端(80+ 端点类型)
  app.tsx             ← 根组件:双栏布局(侧栏 + 对话)
  main.tsx            ← Preact 挂载
  settings.tsx        ← 主题/语言/模型设置
  i18n.ts             ← 国际化
  components/
    Chat.tsx          ← 对话主视图(流式渲染 + 工具调用 + 搜索)
    Sidebar.tsx       ← 会话列表 + 项目树 + Skills 菜单
    ModelSelector.tsx ← 模型选择器
    ModeSelector.tsx  ← 审批模式切换(Build/AcceptEdits/Auto/Plan)
    PermissionCard.tsx← 权限确认卡片
    UserInputCard.tsx ← 结构化用户输入卡片
    PolicyInterventionCard.tsx ← 策略干预卡片
    AttachMenu.tsx    ← 附件菜单(@提及 + Skills)
    FilePicker.tsx    ← 文件浏览器
    CwdPicker.tsx     ← 工作目录选择器
    Markdown.tsx      ← Markdown 渲染(DOMPurify + marked)
  lib/
    liveSteer.ts      ← 实时转向(Live Steer)状态机
    chatTerminal.ts   ← 对话生命周期状态机
    todoState.ts      ← Todo 工具状态投影
    toolRows.ts       ← 工具调用行渲染
    slashCommands.ts  ← 斜杠命令解析
    atMention.ts      ← @提及检测
    composerKeyboard.ts ← 键盘交互(Enter 发送 / Shift+Enter 换行)
    notifications.ts  ← 浏览器通知集成
    sessionList.ts    ← 会话列表管理
    historyMessages.ts← 历史消息导出
  styles/
    app.css           ← 主样式
    theme.css         ← 主题变量
```

#### 2.1.2 核心设计:daemon 内嵌 + rust-embed

Web UI 的静态资源在**编译期**通过 `rust-embed` 嵌入 daemon 二进制:

`crates/atomcode-daemon/src/webui.rs:15-18`:
```rust
#[derive(RustEmbed)]
#[folder = "../../webui/dist/"]
#[allow_missing = true]
pub struct WebuiAssets;
```

运行期 `GET /` 和所有未匹配的非 API 路径都回退到 `index.html`(SPA 路由):

`crates/atomcode-daemon/src/webui.rs:37-43`:
```rust
pub fn asset_or_index(path: &str) -> Option<std::borrow::Cow<'static, [u8]>> {
    let p = path.trim_start_matches('/');
    if let Some(f) = WebuiAssets::get(p) {
        return Some(f.data);
    }
    WebuiAssets::get("index.html").map(|f| f.data)
}
```

**dev 模式**支持热更新:设置 `ATOMCODE_WEBUI_DEV=http://localhost:5173` 后重定向到 Vite dev server:

`crates/atomcode-daemon/src/webui.rs:49-52`:
```rust
if let Ok(dev) = std::env::var("ATOMCODE_WEBUI_DEV") {
    let target = format!("{}{}", dev.trim_end_matches('/'), uri.path());
    return axum::response::Redirect::temporary(&target).into_response();
}
```

#### 2.1.3 核心设计:多客户端实时同步(Live Hub)

daemon 内部有一个 `LiveViewHub`,通过 `broadcast` channel 实现多客户端(WebUI 多标签 + TUI)实时同步:

`crates/atomcode-daemon/src/live_hub.rs:1-11`:
```rust
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use atomcode_coding::{
    CodingRuntimeEvent, CodingRuntimeHandle, DriverCommand, RuntimePhase, RuntimeStatus,
    ...
};
use tokio::sync::broadcast;

const BROADCAST_CAPACITY: usize = 1024;
```

`LiveViewEvent` 枚举定义了所有同步事件:

`crates/atomcode-daemon/src/live_hub.rs:46-68`:
```rust
pub enum LiveViewEvent {
    InputAccepted { input: UserInput, client_input_id: Option<String> },
    Steered { count: usize, inputs: Vec<SteeredInput>, client_input_ids: Vec<Option<String>> },
    CommandOutput(String),
    RequestResolved { request_id: RequestId, kind: String },
    Runtime(CodingRuntimeEvent),
}
```

**嵌入式绑定**:TUI 进程内的 daemon 通过 `register_embedded_runtime()` 注册运行时,WebUI 通过 `streamLive()` 订阅:

`crates/atomcode-daemon/src/native_live.rs:32-64`:
```rust
pub fn register_embedded_runtime(
    session_id: String, working_dir: PathBuf,
    provider: String, provider_fingerprint: String,
    snapshot: SessionSnapshot, control: Arc<dyn LiveRuntimeControl>,
) -> Result<LiveBinding, HubError> {
    // ...
    let binding = hub().bind_with_provider(
        session_id, working_dir, provider, provider_fingerprint, snapshot, control,
    )?;
    *EMBEDDED_BINDING.lock()... = Some(binding.clone());
    crate::live_set_working_dir(binding.working_dir.clone());
    Ok(binding)
}
```

#### 2.1.4 核心设计:Live Steer 实时转向

WebUI 支持在 AI 正在生成时**实时注入新输入**(称为 "steer"),前端有完整的状态机管理:

`webui/src/lib/liveSteer.ts:94-100`:
```typescript
export function reconcileSteerReceipt(
  disposition: SteerReceiptDisposition,
  lifecycle: { running: boolean; terminalConsumed: boolean },
): SteerReceiptOutcome {
  if (disposition === 'started') return 'clear';
  return lifecycle.terminalConsumed ? 'release' : 'confirm';
}
```

三种状态:
- `started` → 新回合开始,清空 pending
- `steered` + 未消费终态 → `confirm`,等待 fold 确认
- `steered` + 已消费终态 → `release`,由运行时重新执行

#### 2.1.5 核心设计:四种审批模式

`crates/atomcode-daemon/src/live_api.rs:28-37`:
```rust
pub(crate) fn fallback_approval_decision(mode: ApprovalMode) -> PermissionDecision {
    match mode {
        ApprovalMode::AcceptEdits | ApprovalMode::Plan => PermissionDecision::Deny,
        ApprovalMode::Build | ApprovalMode::Auto => PermissionDecision::AllowOnce,
    }
}
```

四种模式:
- **Build**: 交互式审批(默认),每个敏感操作需用户确认
- **AcceptEdits**: 文件编辑自动通过,bash 等仍需确认
- **Auto**(bypass): 全自动,所有操作直接通过
- **Plan**: 只读探索,不允许执行

#### 2.1.6 核心设计:乐观会话 + URL 恢复

`webui/src/app.tsx:36-38`:
```typescript
const [optimisticSession, setOptimisticSession] =
  useState<SessionMetaWithProject | null>(null);
```

首条消息发出瞬间用前 10 字做临时标题,乐观插入侧栏(即时可见);后端回传真实 session_id 后按 id 去重覆盖。URL 中存储 session id 前 8 位,刷新后通过 `resolveSession()` 跨所有桶定位完整记录:

`webui/src/api.ts:316-325`:
```typescript
export async function resolveSession(id: string): Promise<SessionMetaWithProject | null> {
  const resp = await fetch(`/sessions/resolve/${encodeURIComponent(id)}`, {
    headers: authHeaders(),
  });
  if (resp.status === 404) return null;
  if (!resp.ok) throw new Error(`resolve session failed: ${resp.status}`);
  return resp.json();
}
```

#### 2.1.7 核心设计:通知系统 + PWA

`webui/src/lib/notifications.ts` 集成了浏览器通知 API,回合完成时推送通知。支持 PWA manifest:

`webui/public/manifest.webmanifest` 提供安装到桌面的能力。

---

### 2.2 extensions/ — IDE 扩展 (高价值,深挖)

#### 2.2.1 VSCode 扩展

**架构**: daemon 进程管理 + webview-ui 通信。扩展本身是薄壳,核心逻辑在 daemon 中。

`extensions/vscode/src/extension.ts:22-31`:
```typescript
export async function activate(context: vscode.ExtensionContext) {
  const config = getConfig();
  extensionState.client = new DaemonClient(config.daemonPort);
  extensionState.daemonProcess = new DaemonProcess(
    extensionState.client, context.extensionUri,
    { defaultPort: config.daemonPort, binaryPath: config.binaryPath, autoStart: config.autoStart },
  );
```

**关键特性**:
1. **自动重连**: daemon 空闲超时退出后,下次请求自动重启(共享 promise 防并发)
2. **CodeAction 提供者**: 右键菜单 explain/fix/optimize,自动注入编辑器上下文
3. **WebviewPanel 序列化器**: 跨重启恢复 Tab 状态
4. **30 秒健康检查**: 定时探测 daemon 存活

`extensions/vscode/src/extension.ts:36-42`:
```typescript
let reconnecting: Promise<boolean> | null = null;
extensionState.client.onConnectionLost = async () => {
  if (!reconnecting) {
    reconnecting = extensionState.daemonProcess.ensureRunning()
      .finally(() => { reconnecting = null; });
  }
  return reconnecting;
};
```

**webview-ui**: 使用 React(not Preact)+esbuild 构建,独立于 webui/ 目录:
```
extensions/vscode/webview-ui/src/
  App.tsx             ← 主应用
  components/         ← UI 组件
  state/              ← 状态管理
  lib/                ← 业务逻辑
  utils/              ← 工具函数
  vscode.ts           ← VSCode API 桥接
```

**l10n 国际化**: 支持中英文(`package.nls.zh-cn.json`)。

#### 2.2.2 JetBrains 扩展

**架构**: Kotlin + IntelliJ Platform SDK,直接嵌入 daemon 二进制。

`extensions/jetbrains/build.gradle.kts:120-126`:
```kotlin
val daemonTargets = listOf(
    DaemonTarget("darwin-arm64", "atomcode-daemon", "ATOMCODE_DAEMON_DARWIN_ARM64", ...),
    DaemonTarget("darwin-x64", "atomcode-daemon", "ATOMCODE_DAEMON_DARWIN_X64", ...),
    DaemonTarget("linux-x64", "atomcode-daemon", "ATOMCODE_DAEMON_LINUX_X64", ...),
    DaemonTarget("linux-arm64", "atomcode-daemon", "ATOMCODE_DAEMON_LINUX_ARM64", ...),
    DaemonTarget("win32-x64", "atomcode-daemon.exe", "ATOMCODE_DAEMON_WIN32_X64", ...),
)
```

**官方构建验证**: JetBrains 插件要求 daemon 必须经过官方签名(`build-official.sh`),否则拒绝启动:

`extensions/jetbrains/build.gradle.kts:133-165`:
```kotlin
val verifyOfficialDaemonForRunIde by registering {
    val daemon = repoRoot.resolve("target/release/$executable")
    // ...
    val process = ProcessBuilder(daemon.toAbsolutePath().toString(), "--check-official-build")
        .start()
    if (process.exitValue() != 0) {
        throw GradleException("does not contain the official AtomGit signer")
    }
}
```

**源码结构**:
```
extensions/jetbrains/src/main/kotlin/com/atomcode/jetbrains/
  daemon/             ← DaemonSupervisor + DaemonClient + ApiClient
  ui/                 ← ChatPanel + ToolWindow + StatusBarWidget
  ide/                ← EditorContext + DiffService + ClipboardService
  services/           ← ProjectService + LoginCoordinator + StartupActivity
  security/           ← TokenFactory + SecretRedactor + SensitivePathClassifier
  settings/           ← Configurable + SettingsState
  persistence/        ← ProjectWorkspaceModels
  diagnostics/        ← Diagnostics
```

**亮点**: JetBrains 扩展有独立的 `SensitivePathClassifier`(敏感路径分类器)和 `SecretRedactor`(密钥脱敏),安全意识强。

---

### 2.3 evals/ — 评估框架 (高价值,深挖)

#### 2.3.1 整体架构

`evals/deepseek-v4-flash/` 是一个**配对评估框架**,对比两个候选模型(AtomGit 官方 vs 火山引擎)在同一组 case 上的表现。

**设计原则**:
- **配对同时启动**: 两个候选在同一个 case 上同时运行,消除时间偏差
- **盲评**: 评估者(Codex)不知道哪个是 A/B,消除偏见
- **多层验证**: 机器验证(脚本) + LLM 盲评(Codex) + 统计分析(bootstrap CI)

#### 2.3.2 评估流水线

`evals/deepseek-v4-flash/eval.py:608-621`:
```python
def main() -> int:
    args = parse_args(); suite = load_suite(args.suite); cases = discover_cases(ROOT/"cases")
    if args.command == "prepare":
        print(prepare(suite, cases, args.results, selected, args.repetitions))
    elif args.command == "run": asyncio.run(run_all(suite, cases, args.run_dir))
    elif args.command == "judge": judge(suite, cases, args.run_dir)
    elif args.command == "summarize": print(json.dumps(summary(args.run_dir), ...))
    elif args.command == "report": report(suite, cases, args.run_dir)
    elif args.command == "combined-report": combined_report(suite, model_run, agent_run, output)
```

五步流水线: `prepare` → `run` → `judge` → `summarize` → `report`

#### 2.3.3 Case 结构

**两层 case**:
- **model 层**(20 case): 纯文本能力(调试/逻辑/代码/指令遵循/上下文/tool schema),无工具调用
- **agent 层**(8 case): 带 fixture 的工具调用(修 bug/跨文件/重构/诊断/契约修复)

`evals/deepseek-v4-flash/cases/agent-cases.json:1-2`:
```json
{"id":"agent-fix-cache","fixture":"agent-fixture",
 "verify":["python3","-m","unittest","tests.test_cache","-v"],
 "prompt":"Fix the TTL cache bug exposed by tests.test_cache...",
 "rubric":{"correctness":"Target test passes; expiration uses monotonic time consistently."}}
```

每个 agent case 有:
- `fixture`: 拷贝的工作目录
- `verify`: 可执行的验证命令(test suite)
- `rubric`: 评分标准

#### 2.3.4 盲评机制

`evals/deepseek-v4-flash/eval.py:471-498`:
```python
def build_packets(suite, cases, run_dir):
    # ...
    for pair in sorted((run_dir/"raw").glob("*/pair.json")):
        # 随机分配 A/B 别名
        rng.shuffle(names)
        aliases = {"A": names[0], "B": names[1]}
        # 删除所有能识别候选的信息
        for value in sorted((x for x in forbidden if x), key=len, reverse=True):
            encoded = encoded.replace(value, "[CANDIDATE]")
        # 验证无泄漏
        leaked = [value for value in forbidden if value and value in encoded]
        if leaked: raise ValueError("provider identity leaked into judge packet")
```

#### 2.3.5 统计分析

`evals/deepseek-v4-flash/eval.py:383-388`:
```python
def bootstrap_ci(values, seed, samples=10000):
    if not values: return None
    rng = random.Random(seed); n = len(values)
    means = [sum(values[rng.randrange(n)] for _ in range(n))/n for _ in range(samples)]
    return [percentile(means, .025), percentile(means, .95)]
```

使用 bootstrap 置信区间(10000 次重采样)评估配对差异的统计显著性。

#### 2.3.6 评估维度

judge prompt 中定义了 4 个评分维度:
- `correctness`: 正确性
- `quality`: 代码质量
- `instruction_following`: 指令遵循
- `agent_execution`: Agent 执行质量(工具使用)

---

### 2.4 docker/ — 容器化部署 (中等价值)

#### 2.4.1 双镜像策略

| 镜像 | 用途 | 基础镜像 |
|------|------|----------|
| Dockerfile-Daemon | NAS/服务器常驻 daemon | debian:bookworm-slim |
| Dockerfile-TUI | macOS/Windows 体验 Linux 版 | debian:bookworm-slim |

**Daemon 镜像关键设计**:

`docker/Dockerfile-Daemon:20-30`:
```dockerfile
COPY dist/v*/atomcode-daemon-*-linux-* /tmp/
ARG TARGETARCH
RUN if [ "$TARGETARCH" = "arm64" ]; then
        mv /tmp/atomcode-daemon-*-linux-arm64 /usr/local/bin/atomcode-daemon;
    else
        mv /tmp/atomcode-daemon-*-linux-x64 /usr/local/bin/atomcode-daemon;
    fi
```

支持 `TARGETARCH` 自动选择 x64/arm64,配合 `build-multiarch.sh` 可构建多架构镜像。

#### 2.4.2 docker-compose 安全设计

`docker/docker-compose.yml:44-80`:
```yaml
ports:
  - "${BIND_ADDR:-127.0.0.1}:13456:13456"  # 默认仅本机
volumes:
  - ./data:/root/.atomcode                   # 持久化数据
  - ./config.toml:/root/.atomcode/config.toml:ro  # 配置只读
  - ./projects:/workspace                    # 项目代码
security_opt:
  - no-new-privileges:true                   # 禁止提权
healthcheck:
  test: ["CMD", "bash", "-c", "exec 3<>/dev/tcp/127.0.0.1/13456 && ..."]
```

**亮点**:
1. 默认绑定 127.0.0.1,必须显式 `BIND_ADDR=0.0.0.0` 才对外开放
2. 配置文件只读挂载,防止容器意外改写
3. `no-new-privileges` 禁止提权
4. 健康检查用 `/dev/tcp` 而非 curl(镜像无 curl)

---

### 2.5 scripts/ — 构建与发布 (中等价值)

#### 2.5.1 release.sh: 六目标交叉编译

`scripts/release.sh:95-166` 编译 6 个平台:
1. macOS ARM (aarch64-apple-darwin)
2. macOS Intel (x86_64-apple-darwin)
3. Linux x64 (x86_64-unknown-linux-musl)
4. Linux ARM64 (aarch64-unknown-linux-musl)
5. Windows x64 (x86_64-pc-windows-gnu)
6. Windows ARM64 (aarch64-pc-windows-gnullvm)

**版本来源**: 从 `Cargo.toml` 的 `[workspace.package].version` 读取,而非 git tag:

`scripts/release.sh:22-28`:
```bash
CARGO_VERSION=$(awk -F'"' '
    /^\[workspace\.package\]/ { in_section = 1; next }
    /^\[/ { in_section = 0 }
    in_section && /^version *=/ { print $2; exit }
' Cargo.toml)
```

**latest.json 生成**: 自动为 `/upgrade` 自更新生成 manifest:

`scripts/release.sh:193-249`:
```bash
{
    printf '"version": "%s",\n' "$VERSION"
    printf '"binaries": {\n'
    for pair in \
        "darwin-arm64:atomcode-${VERSION}-darwin-arm64" \
        "linux-x64:atomcode-${VERSION}-linux-x64" \
        ...
```

#### 2.5.2 release-daemon.sh: daemon 专用构建

专门为 VSCode 扩展打包 daemon 二进制,与 `release.sh` 分离(daemon 默认不在发布包中):

`scripts/release-daemon.sh:80-98`:
```bash
echo "=== AtomCode Daemon Release ${VERSION} ==="
echo "Building webui frontend..."
(cd webui && npm ci && npm run build)
# 必须先构建 webui,否则嵌入的二进制会 404
if [ ! -f webui/dist/index.html ]; then
  echo "error: webui build produced no dist/index.html" >&2
  exit 1
fi
```

#### 2.5.3 其他脚本

| 脚本 | 用途 |
|------|------|
| `sign-macos.sh` | macOS 二进制签名 + 公证 |
| `install.sh` / `install.ps1` | 用户安装脚本(Linux/macOS/Windows) |
| `uninstall.sh` / `uninstall.ps1` | 卸载脚本 |
| `test-all.sh` | 全量测试(cargo test --workspace) |
| `test-headless.sh` | 无头模式测试 |
| `smoke-test-all.sh` | 冒烟测试 |
| `setup.sh` | 开发环境初始化 |
| `acp_smoke.py` | ACP 协议冒烟测试 |
| `analyze_datalogs.py` | 数据日志分析 |
| `gen_mascot.py` | 吉祥物图片生成 |
| `homebrew/scripts/package-tar-gz.sh` | Homebrew 打包 |

---

### 2.6 site/ — 文档站 (低价值,简写)

静态文档站,纯 HTML + Tailwind CSS,支持中英文双语:

```
site/docs/
  en/                 ← 英文文档(20+ 页面)
  zh/                 ← 中文文档(20+ 页面)
  docs.js             ← 交互逻辑(搜索/导航)
  docs.css            ← 样式
  search-index.en.json← 英文搜索索引
  search-index.zh.json← 中文搜索索引
```

**搜索索引构建**: `site/build-search-index.mjs` 从 HTML 页面提取 h1-h3 标题 + 正文,生成 JSON 索引,同时将 `#id` 锚点注入回 HTML:

`site/build-search-index.mjs:125-137`:
```javascript
function injectHeadingIds(mainHtml, heads) {
  let out = '', last = 0;
  for (const h of heads) {
    out += mainHtml.slice(last, h.start);
    out += h.hadId
      ? mainHtml.slice(h.start, h.end)
      : `<${h.tag}${h.attrs} id="${h.id}">${h.headingHtml}</${h.tag}>`;
    last = h.end;
  }
  return out + mainHtml.slice(last);
}
```

---

### 2.7 packages/ — npm 分发 (低价值,简写)

npm 包采用**分平台子包**策略:

`packages/npm/bin/atomcode.js:8-23`:
```javascript
const PLATFORM_PACKAGES = {
  darwin: { arm64: "@atomgit.com/atomcode-darwin-arm64", x64: "@atomgit.com/atomcode-darwin-x64" },
  linux:  { arm64: "@atomgit.com/atomcode-linux-arm64",  x64: "@atomgit.com/atomcode-linux-x64" },
  win32:  { x64: "@atomgit.com/atomcode-win32-x64" },
  ohos:   { arm64: "@atomgit.com/atomcode-ohos-arm64" },
};
```

wrapper 脚本检测当前平台,`require.resolve` 对应子包,然后 `spawn` 真实二进制。

### 2.8 examples/hooks/ — Hook 示例 (低价值,简写)

`examples/hooks/hooks.toml` 展示了三种 hook 触发方式:
- `post_tool`: 工具执行后(如 code_review.sh)
- `post_turn`: 对话回合后(如 auto_commit.sh)
- `pre_tool`: 工具执行前(如 Python hook)

---

## 三、被遗漏 crate 补全

### 3.1 atomcode-codingplan-crypto — 占位签名 crate

`crates/atomcode-codingplan-crypto/src/lib.rs:1-11`:
```rust
//! Open-source placeholder. The real signing crate is overlaid by the
//! official build (scripts/build-official.sh); this stub exists only so the
//! public workspace compiles and carries no signing logic.

pub const ALGORITHM_VERSION: u8 = 0;

pub fn sign_v1(...) -> Vec<(&'static str, String)> {
    unreachable!("request signing requires the official build")
}
```

**关键发现**: 这是一个**占位 crate**,open-source 编译时只是 stub;official build 通过 `build-official.sh` 覆盖整个文件,注入真正的签名逻辑。这是 AtomCode 区分开源版和官方版的核心机制之一。

### 3.2 atomcode-clix — 独立 CLI (atomcodex)

`crates/atomcode-clix/src/main.rs:1-13`:
```rust
//! `atomcodex` — a standalone, single-capability CLI: code review.
//! It drives the `atomcode-review` agent (kernel + capabilities,
//! no atomcode-core/atomcode-cli coupling) over a `git diff`,
//! then prints the structured findings the agent reported.
//!
//! Usage:
//!   atomcodex review [--base <ref>] [--staged] [--repo <dir>] [--model <m>] [--json]
```

三个子命令:
- `code`: 完整交互式 coding agent(组装 tools+codeintel+skills+mcp+session+memory)
- `sessions`: 列出可恢复的会话
- `review`: 代码审查(只读,无 atomcode-core 依赖)

**与主 CLI 的关系**: clix 是**独立二进制**,不依赖 atomcode-core/coding,直接组装 kernel + capabilities,适合嵌入 CI/CD 或作为轻量工具。

### 3.3 atomcode-updater — 自更新机制

`crates/atomcode-updater/src/lib.rs:1-24`:
```rust
//! In-place binary upgrade for atomcode.
//!
//! Flow:
//! 1. Fetch `latest.json` manifest (version + per-target sha256/size).
//! 2. Detect current platform and pick the matching binary entry.
//! 3. Verify we can write to `current_exe()`'s directory.
//! 4. Download the binary to a sibling temp file, streaming progress.
//! 5. Verify SHA256 against the manifest.
//! 6. Three-way swap:
//!    a. `atomcode` → `.atomcode.rolling`
//!    b. new binary → `atomcode`
//!    c. `.atomcode.rolling` → `.atomcode.bak`
```

**三路交换**: 使用 `.rolling` 中间文件和 `.bak` 备份,确保升级失败时可回滚(``/upgrade rollback``)。SHA256 校验在替换活二进制之前完成,绝不先改后验。

### 3.4 atomcode-review — 代码审查 L2 层

`crates/atomcode-review/src/lib.rs:1-16`:
```rust
//! The REVIEW specialization. Assembles the neutral kernel +
//! capabilities into a runnable, READ-ONLY code-review agent
//! that reports structured findings — with ZERO `atomcode-core` involvement.
//!
//! L2 owns:
//! 1. Assembly — build_review_agent: wires provider + the read-only review toolset
//!    (read/grep/glob/ast_grep/codeintel/web_search) + the report_finding sink + the
//!    reviewer persona into a kernel Agent.
//! 2. Persona — review_persona: the reviewer system prompt.
```

子模块:
- `diff.rs`: diff 行号标注
- `fanout.rs`: 并行审查扇出
- `impact_plan.rs`: 影响面分析
- `persona.rs`: 审查者人设
- `rules.rs`: 规则引擎
- `confine.rs`: 范围约束
- `review_tool.rs`: 作为 SubAgent 工具挂载
- `round_budget.rs`: 回合预算控制

### 3.5 atomcode-daemon — HTTP API Server (补全)

前三档文档仅提及 daemon 的存在,未深入其架构。daemon 是整个系统的**中枢**:

**模块职责**:
| 模块 | 职责 |
|------|------|
| `lib.rs` | axum Router + 80+ 端点 |
| `live_hub.rs` | 多客户端实时同步 hub |
| `live_api.rs` | `/live` SSE + `/chat` turn 构建 |
| `native_live.rs` | 嵌入式 TUI ↔ daemon 绑定 |
| `permission_bridge.rs` | HTTP 权限决策桥接 |
| `kernel_runtime.rs` | 运行时生命周期管理 |
| `runtime_host.rs` | 插件 hook + Skill + 限速源 |
| `auth_token.rs` | WebUI token 存储 |
| `daemon_token_file.rs` | daemon token 持久化 |
| `api_auth.rs` | OAuth 登录流程 |
| `api_config.rs` | 配置端点 |
| `api_provider.rs` | Provider CRUD |
| `api_codingplan.rs` | CodingPlan 集成 |
| `approval_mode.rs` | 审批模式管理 |
| `webui.rs` | 内嵌静态资源服务 |
| `telemetry_scope.rs` | 遥测范围 |
| `legacy_convert.rs` | 旧版消息格式转换 |

**SessionMode 枚举**:

`crates/atomcode-daemon/src/main.rs:112-118`:
```rust
let mode = match client_mode.as_deref() {
    Some("vscode") => SessionMode::Vscode,
    Some("jetbrains") => SessionMode::Jetbrains,
    Some("webui") => SessionMode::Webui,
    Some("atomcode-air") => SessionMode::AtomcodeAir,
    _ => SessionMode::Ide,
};
```

**空闲超时**: daemon 默认 30 分钟无活动自动退出,最小 60 秒:

`crates/atomcode-daemon/src/main.rs:18-19`:
```rust
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 30 * 60;
```

---

## 四、对 laew 借鉴路线

### P0 — 当前版本可借鉴

| 借鉴项 | 来源 | laew 现状 | 建议 |
|--------|------|-----------|------|
| **Eval 框架设计** | `evals/deepseek-v4-flash/eval.py` | 无 eval 系统 | 从 paired eval 起步:model 层 10 case + agent 层 5 case,用 LLM 盲评 |
| **Hook 示例** | `examples/hooks/hooks.toml` | 无 hook 系统 | 参考 `post_tool` / `post_turn` 触发器设计 |
| **latest.json 自更新 manifest** | `scripts/release.sh:193-249` | 无自更新 | 参考 manifest 格式 + SHA256 校验 |
| **npm 分平台 wrapper** | `packages/npm/bin/atomcode.js` | 无 npm 分发 | 参考 `PLATFORM_PACKAGES` + `spawn` 透传 |

### P1 — 中期版本可借鉴

| 借鉴项 | 来源 | laew 现状 | 建议 |
|--------|------|-----------|------|
| **WebUI 内嵌二进制** | `webui.rs` + `rust-embed` | 无 Web UI | 若做 Web UI,参考 rust-embed 编译期嵌入 + SPA 回退 |
| **多客户端实时同步** | `live_hub.rs` broadcast channel | 单进程 TUI | 若做 Web UI,参考 LiveViewHub + LiveBinding |
| **审批模式四态** | `live_api.rs` ApprovalMode | 无审批分层 | 参考 Build/AcceptEdits/Auto/Plan 四态,按工具类型分流 |
| **独立 Review CLI** | `atomcode-clix` | 单二进制 | 参考 clix 不依赖 core 直接组装 kernel+capabilities 的模式 |
| **Docker 双镜像** | `docker/` | 无容器化 | 参考 daemon 常驻 + TUI 体验 两套镜像 |

### P2 — 远期版本可借鉴

| 借鉴项 | 来源 | laew 现状 | 建议 |
|--------|------|-----------|------|
| **VSCode/JetBrains 扩展** | `extensions/` | 无 IDE 集成 | 参考 daemon 进程管理 + webview 通信 + 自动重连 |
| **Live Steer 实时转向** | `liveSteer.ts` | 无中途注入 | 参考 `reconcileSteerReceipt` 三态(确认/释放/清空) |
| **乐观会话 + URL 恢复** | `app.tsx` optimisticSession | 无 Web UI | 参考短 id URL + `resolveSession` 跨桶定位 |
| **占位 crate + 官方覆盖** | `codingplan-crypto` | 全部开源 | 若需区分社区版/商业版,参考 stub crate + official build overlay |
| **三路交换自更新** | `updater/lib.rs` | 无自更新 | 参考 `.rolling` + `.bak` 三路交换 + rollback |

---

## 五、总结

本轮深挖覆盖了 8 个此前几乎未被触及的目录和 5 个被遗漏的 crate,揭示了 AtomCode 的**外围生态系统**:

1. **WebUI 是全功能 SPA**,不是简单的管理面板——它有完整的对话、审批、转向、通知、PWA 能力,通过 rust-embed 嵌入 daemon 二进制
2. **IDE 扩展是薄壳 + daemon 模式**,VSCode 和 JetBrains 都不自己跑 AI 逻辑,而是连接同一个 daemon 进程
3. **评估框架设计精巧**,配对启动 + 盲评 + bootstrap CI,是模型选型的工程化方法
4. **codingplan-crypto 的占位模式**揭示了开源/官方版本分离的机制
5. **daemon 是整个系统的中枢**,WebUI/VSCode/JetBrains/TUI 四种前端都连接同一个 daemon,通过 LiveViewHub 实现实时同步

---

## 附录:深读源文件清单(20+)

1. `webui/src/api.ts` — REST/SSE API 客户端(1230 行)
2. `webui/src/app.tsx` — 根组件(550 行)
3. `webui/src/components/Chat.tsx` — 对话主视图
4. `webui/src/components/Sidebar.tsx` — 侧栏
5. `webui/src/lib/liveSteer.ts` — Live Steer 状态机
6. `webui/src/lib/chatTerminal.ts` — 对话生命周期状态机
7. `webui/vite.config.ts` — Vite 构建配置
8. `webui/scripts/mock-live-server.mjs` — Mock daemon 服务器
9. `extensions/vscode/src/extension.ts` — VSCode 扩展入口
10. `extensions/vscode/package.json` — VSCode 扩展配置
11. `extensions/jetbrains/build.gradle.kts` — JetBrains 构建脚本
12. `evals/deepseek-v4-flash/eval.py` — 评估框架(627 行)
13. `evals/deepseek-v4-flash/cases/agent-cases.json` — Agent 评估用例
14. `evals/deepseek-v4-flash/cases/model-cases.json` — Model 评估用例
15. `evals/deepseek-v4-flash/benchmark.json` — 基准配置
16. `evals/deepseek-v4-flash/prompts/codex-judge.md` — 盲评 prompt
17. `evals/deepseek-v4-flash/tests/test_eval.py` — 评估框架测试
18. `docker/Dockerfile-Daemon` — Daemon Docker 镜像
19. `docker/docker-compose.yml` — Docker Compose 部署
20. `scripts/release.sh` — 六目标发布脚本(258 行)
21. `scripts/release-daemon.sh` — daemon 专用发布
22. `crates/atomcode-daemon/src/lib.rs` — daemon 主入口(350KB)
23. `crates/atomcode-daemon/src/live_hub.rs` — 多客户端同步 hub
24. `crates/atomcode-daemon/src/native_live.rs` — 嵌入式绑定
25. `crates/atomcode-daemon/src/live_api.rs` — Live API
26. `crates/atomcode-daemon/src/permission_bridge.rs` — 权限桥接
27. `crates/atomcode-daemon/src/webui.rs` — 内嵌资源服务
28. `crates/atomcode-daemon/src/main.rs` — daemon 二进制入口
29. `crates/atomcode-codingplan-crypto/src/lib.rs` — 占位签名
30. `crates/atomcode-clix/src/main.rs` — 独立 CLI
31. `crates/atomcode-updater/src/lib.rs` — 自更新
32. `crates/atomcode-review/src/lib.rs` — 代码审查 L2
33. `crates/atomcode-codingplan/src/client.rs` — CodingPlan 客户端
34. `crates/atomcode-codingplan/src/sync_marker.rs` — 同步标记
35. `crates/atomcode-daemon/src/runtime_host.rs` — 运行时宿主
36. `examples/hooks/hooks.toml` — Hook 配置示例
37. `site/build-search-index.mjs` — 搜索索引构建
38. `packages/npm/bin/atomcode.js` — npm wrapper
