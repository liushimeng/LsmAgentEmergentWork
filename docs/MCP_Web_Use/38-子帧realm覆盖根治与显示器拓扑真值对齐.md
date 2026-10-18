# 38. 子帧 realm 覆盖根治与显示器拓扑真值对齐（第 184 轮）

> 状态：已落地。前置文档：`26`（档位设计）/ `31`（注入时序）/ `33`（注入面收敛）/
> `35`（体检面全覆盖）/ `36`（指针媒体仿真）/ `37`（参数真值对齐）。
> 本文是「子帧 realm 覆盖 + 跨 realm 反查 + 显示器拓扑真值」方向的唯一最新版。

## 一、背景与实证方法

第 174–183 轮把反检测读数从页面侧（webdriver/插件形态/屏幕几何/指针媒体）修到参数侧
（真 GPU/滚动条/Accept-Language），但全部**只在顶层 frame 度量与处置**。本轮先用扩展后的
基线探针（`examples/stealth_baseline_probe.rs`，新增 R184 向量组与 `crossrealm` 模式）
在本机做双档基线 + 子帧覆盖实证，再按实证结果处置。

## 二、真机基线（Windows 11 / Chrome 155 / Intel UHD / 双显示器 / 系统深色模式）

### 2.1 一致项（正向钉死，不处置）

| 向量 | 有头档（真值） | 无头档（新参数） | 结论 |
|------|----------------|------------------|------|
| `prefers-color-scheme` | `dark`（跟随系统） | `dark` | **一致** —— 无头档同样跟随 OS，kills「color-scheme 族需仿真」假设 |
| `forced-colors` / `prefers-reduced-motion` / `prefers-contrast` / `inverted-colors` | none / no-preference / false / false | 同 | 一致 |
| `color-gamut` / `dynamic-range` / `video-dynamic-range` / `update` / `scripting` | sRGB / standard / false / fast / enabled | 同 | 一致 |
| **WebGPU 适配器**（`navigator.gpu.requestAdapter().info`） | `vendor=intel, architecture=gen-12lp` | **同** | **一致** —— 37 轮真 GPU 参数对齐同时覆盖了 WebGPU 独立适配路径，本轮钉死为已验证 |
| **canvas 渲染哈希**（文字+渐变合成 `toDataURL`） | `7262:baa6a523` | `7262:baa6a523` | **逐字节一致** —— 37 轮「Canvas/WebGL 渲染哈希随真硬件走」的端到端实证 |
| Battery / `performance.memory.jsHeapSizeLimit` / `screen.isExtended` 见下 | charging=1,level=1 / 4395630592 | 同 | 一致 |

### 2.2 差异项（本轮处置对象）

| 向量 | 有头档（真值） | 无头档 | 判定 |
|------|----------------|--------|------|
| **`screen.isExtended`**（Window Management API 多显示器标记） | `true`（本机双显示器） | `false` | **显示器拓扑派生读数，无头虚拟屏幕恒 false** —— 与 36 轮指针四元同类的「无头环境读数失真」，可处置 |
| `devicePixelRatio` | 1.25 | 1.0 | 维持 37 轮不处置口径（处置代价>收益，非判别性读数） |
| `screen.avail*` | 2048x1232 | 1920x1080 | 维持不处置（无 CDP 通道，JS 伪造属「形状错误的回填」） |
| `navigator.connection` rtt/downlink | 200/1.75 | 150/1.55 | NetworkInformation 本就时变噪声，非稳定指纹，不处置 |
| `outer==inner` 窗口几何 | false（1906x986） | true（1920x1080） | 维持 179 轮软提示口径 |

## 三、子帧 realm 覆盖缺口（本轮核心实证）

**源码层**：chromiumoxide 0.9.1 生成的 `AddScriptToEvaluateOnNewDocumentParams` **没有
`runForEveryFrame` 字段**（其 .cdp 快照落后于协议），而 CDP 该参数缺省 `false` =
**脚本只注入顶层 frame**。全部隐身注入（`browser_ns::inject_stealth`，含
new_page/自愈/收编/切档/派生页武装五条路径）都踩在这个缺省上。

**实证**（`stealth_baseline_probe crossrealm`，隐身脚本按生产路径注入后自建 srcdoc
同源 iframe 反查，原始数据见 §七）：

| 探测 | 结果 | 含义 |
|------|------|------|
| `iframe_ts_patched` | `false` | 隐身脚本**没有进子帧** —— 子帧 realm 的 `Function.prototype.toString` 是**原生**的 |
| `leak_webdriver_getter` | `function webdriver() { return false; }` | 站点一行 JS 即可读到我们 CORE 覆写的完整源码 |
| `leak_ts_via_iframe` | `function toString() { try { if (__fd.has(this)) …` | 侵入式档原生化包装器自身源码可读（连登记表机制名 `__fd` 都暴露） |
| `leak_query` | permissions.query 包装器**全量源码（含中文注释）** | FIDELITY ⑤ 的改写同样可读 |
| `iframe_webdriver` | `false` | 正向：iframe 内 webdriver 裸值本就为 false（无此泄漏） |
| `top_ts_self` | `[native code]` | 顶层自反正常（既有单测覆盖的形态） |

**攻击成本**：站点风控 JS 三行 —— 自建 `srcdoc` iframe（同源）→
`iframe.contentWindow.Function.prototype.toString.call(顶层被改写函数)` → 拿到非
`[native code]` 源码即判「函数被包装」。这与 `patched_fn_leak`（179 轮，同 realm 逐入口
反查）同类但**完全绕过**了登记表 —— 攻击者借的是一个我们从没处置过的 realm。
174 轮把缺省升到 hardened 的核心理由（BOSS 直聘实测 `patched_functions_visible`
被 geetest 采集加权）在这个路径上**整段失效**。

## 四、处置设计

### 4.1 `runForEveryFrame` 补发（`src/agent/cdp_ext.rs`，新增模块）

- 用 `#[serde(flatten)]` 包一层类型化参数 + `runForEveryFrame: true`，实现
  `chromiumoxide::Method`/`Command`（响应类型复用 `AddScriptToEvaluateOnNewDocumentReturns`），
  `Page::execute` 直接可发 —— 不 fork CDP 客户端、不引新 crate；
- **兼容回退**：命令被老 Chrome 拒绝（未知字段报协议错）时自动回退旧参数重发一次
  （只丢子帧覆盖，不丢顶层注入 —— 任何环境都不比改前差）；
- 收口在 `browser_ns::inject_stealth` 唯一入口，五条注入路径（new_page 首航 / heal 自愈 /
  adopt 收编 / set_stealth 切档 / spawn_arm 派生页武装）自动受益；guard/状态栏/跟随器等
  **视觉类**注入不经此口，维持只进顶层（各脚本本就有「只在顶层 frame 渲染」闸）。

### 4.2 跨 realm `toString` 路由（`HARDENED_JS`）

`runForEveryFrame` 之后子帧也装了原生化，但每帧登记表（`__fd` WeakMap）是**帧局部**的：
内层 toString 反查外层函数时本帧 miss，直接回退 `origToString` 仍会吐源码。修复 =
**miss 时路由回家**：

```js
try {
    if (typeof this === 'function') {
        const home = Object.getPrototypeOf(this).toString; // this 本 realm 的(被处置的)toString
        if (typeof home === 'function' && home !== wrappedToString) return home.call(this);
    }
} catch (e) {}
return origToString.call(this);
```

- **终止性**：每一跳都走到 `this` 自身原型链所在 realm 的 toString；到家后
  `home === wrappedToString` 跳出 —— 最多两跳；
- **零新注入面**：不共享登记表（把表挂到 ns 对象上会让攻击者拿到 symbol 后直接
  `fd.has(fn)` 探测任意函数是否被我们改过 —— 用一个泄漏换另一个泄漏）；
- `typeof this === 'function'` 守卫：非函数 `this` 原生行为是 TypeError，路由到
  `Object.prototype.toString` 会返回 `"[object …]"` 字符串，属行为漂移，必须挡住；
- 真页面函数（未登记）到家后 miss → `home === self` 跳出 → 原生源码，**无误伤**。

### 4.3 `screen.isExtended` 回填（`FIDELITY_JS ⑥` + `src/agent/display_topology.rs`）

- **真值来源（OS）**：Windows `GetSystemMetrics(SM_CMONITORS) > 1`
  （`Win32_Graphics_Gdi` feature 已在册）；macOS `CGGetActiveDisplayList` C FFI
  （CoreGraphics，macos_legacy FFI 同款手法，零新 crate）；Linux 走 `xrandr --listmonitors`
  子进程解析（wmctrl/xdotool 同族），缺工具 fail-open。**进程级 OnceLock 缓存**，
  显示器热插拔需重启进程才生效（文档口径）；
- **门控**：`OS=多显示器 && 页面读 false` 才动作 —— 单显示器机器（无头有头都读 false）
  零动作；有头档原生即真值零动作；connect 零注入契约不变；off 档零注入；
- **形态镜像**：回填前先探测原生属性挂在哪个槽位（`screen` 实例 own /
  `Screen.prototype`）与哪种形态（数据属性 / 存取器），**同位同形**回填 —— 布尔值 +
  镜像描述符，无「形状错误的回填」风险；属性原生缺失的浏览器不凭空补（补了才是签名）；
- 真值标记挂 ns 对象非枚举键 `MD`，供体检层对账（见 4.4）；`__adoptAll` 同步补
  `Screen.prototype.isExtended` 认领（跨档升级认领面收全）；
- 无 CDP 原生通道（`setEmulatedMedia` 不覆盖该 API），故走 JS 回填而非 36 轮仿真路线。

### 4.4 体检补口（`fingerprint.rs`）

- `PROBE_JS` 新增：
  - `crossrealm_tostring_leak`：**创建 iframe 在前、测量在末**（中间隔着 WebGL 上下文
    创建等重活，给子帧注入留时序），自建 srcdoc iframe 反查 `toString` 自身 /
    webdriver getter / permissions.query 三个入口，非 `[native code]` 的点名回报；
  - `screen_is_extended`：`{value, own, proto, md_marker}`（md_marker = ns 对象上的
    OS 真值标记）；
- `classify` 新增：
  - `crossrealm_tostring_leak`：进 `MODE_DESIGN_CODES`（保真档 by_design —— 该档本就不
    改函数身份；**侵入档命中 = runForEveryFrame 未生效或路由失效**，属处置失效真故障）；
  - `is_extended_mismatch`（warn）：md_marker 在场而页面读 false —— 回填失效自检，
    按 178 轮教训不进 HARD_CODES；
- `build()` 顶层回报两个观测字段。

## 五、档位语义与注入面影响

- 脚本进入**全部子帧**（含跨域 iframe）：隐身脚本无视觉元素、不受页面 CSP 约束
  （CDP 注入层级高于 CSP）；跨域 iframe 对应的第三方页面环境同样被规整
  （其 realm 的 webdriver/插件读数与本站一致）；
- 幂等闸按帧独立：各帧有独立 `window`，`Symbol.for` 同键各挂各的命名空间对象，
  `StealthRev` 位掩码语义不变；体检计量的 `injected_ns_symbols` 在顶层读数不变
  （仍为 1）；
- 攻击面收口后的等价性：站点能拿到的**任何**同源 realm 的 toString 都已被处置，
  且互相之间能路由回家 —— 「借一个干净 realm 反查」的路被堵死；跨域 realm 拿不到
  顶层函数引用（同源策略），Worker realm 收不到函数引用（结构化克隆不可传函数），
  均天然无通道；
- connect 模式零注入契约不变；`off` 档零注入不变。

## 六、诚实边界（不变 + 本轮新增）

- 不伪造显示器拓扑：OS 说单显示器就不补 `isExtended`（false 是真值）；
- 不处置 dpr / avail / conn 噪声 / outer==inner（37 轮口径逐条维持）；
- 本轮所有真值来源都是**本机 OS 实读**（显示器数量）或**页面自身读数**（无伪造）；
- Canvas/字体伪造、行为仿真、IP 代理、打码、协议逆向依旧一律不做；被拦时的唯一
  合规改道仍是 `open(reuse_existing=true)` 接管用户已登录的真实浏览器。

## 七、验证（实测数据）

1. **实证复测**（`stealth_baseline_probe crossrealm`，修复后）：
   `iframe_ts_patched=true`、`leak_webdriver_getter`=`function get webdriver() { [native code] }`
   （注册串形态 —— 响应方是跨 realm 路由回家的登记表，非原生 toString）、
   `leak_ts_via_iframe` / `leak_query` 全部 `[native code]`；
2. **生产探针同步路径**（`PROBE_JS` 同一次 eval 内建 iframe + 末位测量 —— 初始
   about:blank realm，时序最紧的真实攻击面）：`crossrealm_tostring_leak: []` 零泄漏、
   `injected_ns_symbols: 1`（注入面计量不变）、`patched_fn_leak: []`（既有自证零回归）、
   `screen_is_extended: {md_marker: true, proto: true, value: true}`（OS 双显示器真值
   经 FIDELITY_JS ⑥ 回填生效，与有头档一致）；
3. **真机基线**（`stealth_baseline_probe compare`，10 项全 PASS）：既有 7 项之上新增
   `canvas_render_hash`（`7262:baa6a523` 双档逐字节一致）/ `webgpu_adapter`
   （`intel / gen-12lp` 双档一致）/ `prefers_color_scheme`（`cs_dark=true` 双档一致）；
4. **单测**：`cargo test --lib` **2432 passed**（含本轮新增：cdp_ext 序列化形状 /
   display_topology 字面量形状与主流平台真值 / crossrealm 泄漏三分支与保真档
   by_design 分流 / is_extended_mismatch 四分支 / 探针时序契约（创建在前测量在末，
   源码层防顺序倒换）/ 占位符渲染契约 / 原生化跨 realm 路由在位与档位隔离）；
   `fingerprint.rs` 内联测试按拆分规范机械搬移 `fingerprint/tests.rs`，搬移前后
   测试数对账一致（2432）；
5. **真机冒烟**：`cargo test --test stealth_reveal_smoke -- --ignored` 4 用例全绿
   （服务端取证 + 回填替身差分 + 派生页早期武装 —— `inject_stealth` 改造在
   spawn_arm 路径无回归；含 36 号文遗留竞态用例 `stealth_first_document_single_load_smoke`
   本次通过）。
