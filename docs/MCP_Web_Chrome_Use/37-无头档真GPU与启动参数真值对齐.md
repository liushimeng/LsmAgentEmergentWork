# 37. 无头档真 GPU 与启动参数真值对齐（第 183 轮）

> 状态：已落地。前置文档：`26`（档位设计）/ `29`–`36`（形态自洽、注入面收敛、体检面全覆盖、
> 指针媒体仿真）。
> 本文是「启动参数层真值对齐」方向的唯一最新版。

## 一、背景：前 182 轮都在补「页面侧」，本轮补「参数侧」

第 174–182 轮的反检测把页面侧读数一处处对齐真机（webdriver 覆写、插件形态、屏幕几何、
指针媒体四元、触摸点归零……），但**启动参数集本身**还带着旧无头时代（`--headless` 老模式）
的痕迹：无头档强制 `--disable-gpu` + `--hide-scrollbars`。这两个开关在 new headless
（Chrome 与有头同一渲染路径）下不再是必需项，反而亲手制造了新的指纹差：

- `--disable-gpu` 强制软件渲染 → WebGL 报 **WARP**（`Microsoft Basic Render Driver`）；
- `--hide-scrollbars` 强制隐藏滚动条 → 经典滚动条机器上 `innerWidth - clientWidth` 恒 0。

## 二、真机基线（第 183 轮，Windows 11 / Chrome 155 / Intel UHD，全部走生产探针）

采集工具：`cargo run --release --example stealth_baseline_probe -- <mode> [gpu] [sb]`
（本轮已扩展为带取证服务器回灌 + 新向量 + `compare` 双档对账模式）。

| 向量 | 有头档（真值） | 无头·旧参数（disable-gpu + hide-scrollbars） | 无头·新参数（本轮） | 判定 |
|------|----------------|----------------------------------------------|---------------------|------|
| WebGL UNMASKED vendor | `Google Inc. (Intel)` | `Google Inc. (Microsoft)` | `Google Inc. (Intel)` | **核心差 → 已消除** |
| WebGL UNMASKED renderer | `ANGLE (Intel, Intel(R) UHD Graphics (0x00004688) Direct3D11 vs_5_0 ps_5_0, D3D11)` | `ANGLE (Microsoft, Microsoft Basic Render Driver (0x000008C) Direct3D11 …)` | 与有头**逐字节一致** | **核心差 → 已消除** |
| `WEBGL_debug_renderer_info` | 在场 | 在场（WARP 串） | 在场（真硬件串） | 一致 |
| 滚动条宽度（`innerWidth - clientWidth`） | `0`（Win11 overlay 滚动条） | `0`（被 flag 压掉） | `0` | 本机一致；**经典滚动条机器上旧参数必差 17px，新参数跟随宿主默认** |
| `speechSynthesis.getVoices().length` | `0` | `0` | `0` | 双档一致（Chrome/Windows 无本地语音是平台现状），**不处置** |
| `mediaDevices.enumerateDevices()` | `3` | `3` | `3` | 一致，不处置 |
| `AudioContext.state` | `suspended` | `suspended` | `suspended` | 一致（真实页面无手势同样 suspended），不处置 |
| `Accept-Language`（服务端取证） | `zh-CN,zh;q=0.9` | `zh-CN,zh;q=0.9` | `zh-CN,zh;q=0.9` | 一致 —— Chrome 全新 profile 从 OS 区域设置种子，**无需 `--lang`** |
| `navigator.connection` | `4g/150/1.5` | `4g/150/1.5` | 同 | 一致 |
| `hardwareConcurrency` / `deviceMemory` | `24` / `16` | `24` / `16` | 同 | 一致 |
| `screen.orientation` | `landscape-primary` | `landscape-primary` | 同 | 一致（35 轮观察项维持防御性观测） |

### 已知残差（明确不处置，见 §五）

| 向量 | 有头（真值） | 无头（新参数） | 不处置理由 |
|------|--------------|----------------|------------|
| `devicePixelRatio` | `1.25`（150% 缩放） | `1.0` | 把 DPR 仿真成 1.25 会把 CSS 视口缩成 1536×864，全工具链坐标/布局/截图数学全部失真；且 100% 缩放的外接显示器是常态人群，非判别性读数 |
| `screen.avail*` | `2048x1232`（扣任务栏） | `1920x1080`（=声明的 screen） | CDP `setDeviceMetricsOverride` 没有 avail 字段，无从原生对齐；JS 伪造 avail 是「形状错误的回填」同款风险；`avail == screen` 在自动隐藏任务栏机器上真实存在 |

## 三、处置设计

### 3.1 无头档剔除 `--disable-gpu`（本轮核心）

**为什么 WARP 是指纹**：风控侧对 GPU 串做白名单/软渲染判定（WARP = Windows 高级光栅化
平台 = 无 GPU/虚拟机/被禁用显卡的典型读数），且「无头 + UA 声明 Windows + 报 WARP」
是常见无头画像。此前 `FIDELITY_JS ①` 只对 `webkit/swiftshader/software` 兜底串回填，
**WARP 串三个令牌一个不含**，既不回填也不告警，原样透传给站点。

**处置 = 不处置**：删掉 `--disable-gpu` 后 new headless 用本机真 GPU 初始化 ANGLE，
WebGL vendor/renderer 与同机有头 Chrome **逐字节一致** —— 这是真值，不是伪造，
比任何 JS 层回填都强（Canvas/WebGL 渲染哈希也随真硬件走，与用户日常浏览器同水位）。

**优雅降级**：GPU 初始化失败的机器（VM / RDP / 驱动被禁），Chrome **原生**回退
SwiftShader/WARP —— 与 `--disable-gpu` 时代读数相同，不比改前差；此时
`FIDELITY_JS ①` 的兜底回填与 §3.3 的告警照常兜住。

**杀开关**：`LAEW_WEB_HEADLESS_SOFTRENDER=on|1|true|yes|软渲染`（缺省关）恢复旧参数集
（`--disable-gpu` + `--hide-scrollbars`），供「真 GPU 初始化导致渲染异常」的机器排障对照。
`auto_relaunch` 调试实例（`browser_reuse::relaunch_args`）同策略同步。

### 3.2 无头档剔除 `--hide-scrollbars`

老式无头截图整洁需求的历史遗产。new headless 下移除后滚动条跟随**宿主 OS 默认**：
Win11 Fluent overlay 滚动条机器读数 0（与有头一致，本机实测）；经典滚动条机器
（Win10 等）读数 ~17px（与该机有头 Chrome 一致）。截图在经典滚动条机器上会多出
真实滚动条 —— 那本就是真机形态。

### 3.3 体检补口：WARP 令牌进兜底串判定

`fingerprint::is_default_webgl` 增加 `basic render driver` 令牌（WARP 读数特征串）。
本轮之后无头档仍读到 WARP/SwiftShader 类软渲染串，说明真 GPU 初始化失败
（VM/RDP/驱动问题）——`webgl_default_strings` 告警的 advice 同步改写为第 183 轮口径
（检查是否误开 `LAEW_WEB_HEADLESS_SOFTRENDER`；真无 GPU 环境建议 `reuse_existing`）。

### 3.4 UA 去令牌落地后兜底（修 36 §五遗留的偶发竞态残留通道）

`stealth_first_document_single_load_smoke` 在 Windows 全文件连跑偶发 req#1 带
`HeadlessChrome` 令牌（干净 HEAD 三轮一败复现）。排查到的残留通道：首航前
`apply_ua_destealth` 的探针 eval 若在 about:blank 上超时（冷启动慢/GPU 进程初始化
抢资源，本轮启用真 GPU 后概率上升），返回 `Skipped` 且**无人再补**——goto 就带着
令牌出去了。本轮在 goto 收尾补一道：无头档 + 非 connect 且 `ua_sweep == Skipped`
时，落地后重探一次并下发完整覆盖（页面侧与后续请求全部干净；req#1 已发出的极端
场景不再扩大）。有头档（本就无令牌）与 connect（零注入契约）零动作。

### 3.5 基线探针升级（`examples/stealth_baseline_probe.rs`）

- 默认参数集镜像**新生产参数**；`softrender` 实验开关恢复旧集（A/B 对照）；
- 内置取证服务器把 `Accept-Language`/`Sec-CH-UA` 回灌进页面，页面侧一次 eval 采样；
- 新增向量：滚动条宽度 / `screen.avail*` / DPR / connection / 媒体设备 / 语音列表 /
  AudioContext 状态；
- `compare` 模式：同进程先 headed 后 hidden 双跑，对 WebGL 双串 / 滚动条宽度 /
  指针四元 / 触摸点 / Accept-Language 逐项 PASS/FAIL 对账（本轮验收载体，
  后续改参数集必跑）。

## 四、档位语义与注入面影响

- **两档同时受益**：参数层在档位之下，fidelity/hardened 无差别生效；
- **JS 注入面收缩**：健康 GPU 机器上 WebGL 读数即真值，`FIDELITY_JS ①` 的
  `isFallback` 不再命中 → `getParameter` **零包装**，`patched_fn_leak` 登记面缩短；
  兜底回填仅保留给 GPU 初始化失败的机器（此时它兜的是 SwiftShader/WARP 串）；
- **connect 模式契约不变**：零注入、零参数干预（接管的是用户自己拉起的浏览器）。

## 五、诚实边界（不变 + 本轮新增）

- 不伪造 DPR / avail 几何（§二表二：处置代价 > 收益，且非判别性读数）；
- 不补语音列表 / 媒体设备（双档真值本就一致，编造名单才是制造签名）；
- Canvas/字体伪造、行为仿真、IP 代理、打码、协议逆向依旧一律不做；
- 被拦时的唯一合规改道仍是 `open(reuse_existing=true)` 接管用户已登录的真实浏览器。

## 六、验证

1. **真机基线（本机 Windows 11 / Chrome 155 / Intel UHD）**：
   - `stealth_baseline_probe compare`：WebGL vendor/renderer / 滚动条宽度 / 指针四元 /
     触摸点 / Accept-Language 双档**全项 PASS**（WebGL 双串逐字节一致）；
   - 旧参数集（`softrender`）A/B 复现 WARP 串 → 差分成立。
2. **单测**：`custom_launch_flags` 新参数集与杀开关门控 / `is_default_webgl` WARP 令牌 /
   `relaunch_args` 同步 / UA 兜底路径文案。
3. **真机冒烟**：`stealth_reveal_smoke`（服务端取证 + 回填替身差分 + 派生页早期武装 +
   本轮新增 `stealth_headless_gpu_truth_smoke`：无头档 WebGL 必须报非软渲染真硬件串，
   VM/RDP 无 GPU 机器属预期失败、用例文档注明跳过条件）。
