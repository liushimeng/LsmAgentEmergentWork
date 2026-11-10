# 58 - 浏览器一级菜单栏与 Console 面板设计（DevTools 风格,首个面板 Console）

## 背景与目标

`MCP_Web_LsmAgentBrowser_Use` 打开的自研 `LsmAgentBrowser` 有头查看器窗口此前只有
「工具栏(后退/前进/刷新/首页) + 地址栏 + 页面画面」。本设计为窗口添加:

1. **一级菜单栏**:DevTools 风格的一排一级按钮——
   `Elements` / `Console` / `Sources` / `Network` / `Performance` / `Memory` /
   `Application` / `Security` / `Lighthouse` / `Recorder`。
2. **首个功能面板 `Console`**(Chrome DevTools Console 语义):
   - **分级日志输出**:`log/info/warn/error/debug` 等级别分色;`console.table`
     文本表格、`console.dir` 对象展开、`group/groupEnd` 分组缩进、`clear()` 清空;
   - **调试诊断工具**:`assert` / `count/countReset` / `time/timeLog/timeEnd` /
     `trace`(带调用栈);
   - **实时 JS 交互**:面板底部输入框回车即在**页面全局上下文**执行表达式,
     结果回显;快捷语法 `$_`(上一条返回值)、`$$()`(等效
     `document.querySelectorAll`)、`$0`(最近聚焦/点击元素);
   - **错误自动捕获**:内核既有 console 台账(`__cerror` 链路)原样展示,
     错误红色、警告橙色;
   - **效率辅助**:级别过滤开关、关键词搜索、清空、`Preserve log`
     (跨导航保留历史)。
3. 其余 9 个按钮**先占位**:点击显示「尚未实现」占位面板,点击 `Console` 切回真面板。

浏览器形态对齐真浏览器(Chrome):**Console 面板停靠窗口底部(dock-to-bottom)**,
打开时页面区高度收缩,页面视口随之变小(Chrome 打开 DevTools 的原生行为);
页面永远不会被面板遮住——不存在「显示不全」。

## 总体架构

```
宿主(laew 引擎进程)                                查看器子进程(laew __browser-view)
┌─────────────────────────────┐                  ┌──────────────────────────────────┐
│ 内核 console 台账            │   帧通道 stdin   │ 一级菜单栏(10 个按钮,原生 UI)   │
│  tab.console: Vec<ConsoleRecord>│ ──── 'd' 帧 ──▶│ 工具栏 + 地址栏(既有)           │
│  (level/text/at_ms,上限 400) │  console_sync   │ 页面画面 NSImageView/StretchDIBits│
│                              │  整份快照(JSON) │ Console 面板(原生 UI)           │
│ devtools_console_open 旗标   │ ◀──── 命令通道 ──│  过滤行 + 日志区 + JS 输入框     │
│  push_frame 收尾按需同步      │  devtools/console_eval│                                │
└─────────────────────────────┘                  └──────────────────────────────────┘
```

### 数据流(单一真源:内核 console 台账)

- **同步模型 = 整份快照(幂等)**:宿主把当前页 `tab.console` 全量序列化为
  `{"kind":"console_sync","page_id":..,"entries":[{level,text,at_ms}…]}` 推给查看器;
  查看器按「同页增长追加尾巴 / 缩短或换页整体替换」合并(Preserve log 开启时
  替换改为追加)。不做增量协议——台账上限 400 条,快照开销可忽略,且天然
  免疫乱序/丢帧。
- **推送时机**:① 查看器请求打开 Console(`{"cmd":"devtools","panel":"console"}`);
  ② `push_frame` 收尾(导航/动作/心跳后,凡推帧即同步;仅当面板打开时推送);
  ③ `console_eval` 回写台账后立即推送。
- **eval 回写**:`input`(回显表达式)/ `result`(成功结果)/ `eval_error`(异常)
  三种级别追加进**同一本台账**(`tab.console`),随快照到查看器分色显示。
  `eval_error` **刻意不复用 `error` 级**——`inspect(info=page_health)` 的
  `console_errors` 统计只认 `level=="error"`,人工调试输入的异常不得污染
  Agent 侧的页面错误计数。

### 命令/帧协议(view/mod.rs)

帧通道(宿主 → 查看器)新增 kind:

| kind | 含义 | payload |
| ---- | ---- | ------- |
| `'d'` | DevTools 状态(JSON) | `{"kind":"console_sync","page_id":"p_x","entries":[…]}` |

命令通道(查看器 → 宿主)新增行 JSON:

| 命令 | 含义 |
| ---- | ---- |
| `{"cmd":"devtools","panel":"console"}` | 打开 Console 面板(宿主置 `devtools_console_open` 并立即回推快照) |
| `{"cmd":"devtools","panel":"none"}` | 关闭 DevTools 面板(宿主清旗标,停推) |
| `{"cmd":"devtools","panel":"elements"\|…}` | 其它占位面板(宿主仅回推 `{"kind":"panel_stub","panel":…}` 之类不必须;查看器本地渲染占位即可) |
| `{"cmd":"console_eval","expr":"…"}` | 在显示页执行 JS 表达式,结果回写台账并同步 |

`ViewCommand` 相应新增 `DevTools { panel: String }` 与 `ConsoleEval(String)`。
占位面板的展示**纯查看器本地**(不进宿主),只有 `console`/`none` 需要宿主参与。

## Console 台账能力增强(script/prelude.rs)

既有:`log/info/warn/error/debug/trace/assert/dir/table/time/timeEnd` 中
`dir/table/time/timeEnd` 是空实现,`clear` 是 no-op。补齐:

| API | 实现 |
| ---- | ---- |
| `clear()` | 向批次缓冲推 `__clear` 标记(Rust 合并点识别后清空台账) |
| `dir(x)` | `__clog("log", __str(x))`(对象 JSON 展开) |
| `table(x)` | 文本表格:数组/对象行 × 键列对齐(空格填充,上限 100 行) |
| `group(label)/groupCollapsed/groupEnd` | 组深度计数,后续条目缩进前缀 `▸ ` |
| `count(label)/countReset(label)` | 计数表,输出 `label: n` |
| `time(label)/timeLog(label)/timeEnd(label)` | 计时表(`Date.now()`),输出 `label: nms` |
| `trace()` | 文本 + `new Error().stack` 调用栈 |

**合并点统一化**:`tab.console.push(...)` 的 4 处调用点
(`browser/mod.rs` 导航重建、`api.rs` 三处交互/泵收尾)收敛为
`push_console_logs(tab, logs)` 助手——统一处理 `__clear` 标记(清台账)与
400 条上限,后续新增合并点不再漏标记语义。

## 实时 JS 交互(console_eval)

- 宿主把表达式包一层求值包装(纯函数 `js_preprocess::wrap_console_eval`),
  复用 **`act_eval` 全链路**(页面 JS 线程持久全局优先,线程不可用回退快照桥,
  顶层 return 自动 IIFE 等既有语义全部继承):
  - 快捷语法:包装层临时把 `$$`/`$_`/`$0` 定义到 globalThis(遮蔽站点同名,
    Chrome 控制台同语义),间接 eval(`(0,eval)`)在**页面全局作用域**执行
    (表达式/语句均可,`var`/`function` 声明落全局,与 Chrome 一致),
    求值后恢复站点原值;
  - 返回值经 `__str` 渲染;异常捕获后以 `eval_error` 回写;
- `console_eval` 作用于**查看器正在显示的页**(跟随页优先,`input_target_page`),
  人工看到哪页就调试哪页。

## 查看器 UI 布局(两平台一致)

```
┌────────────────────────────────────────────────────────┐
│ [Elements][Console][Sources][Network][Perf][Mem]…       │ ← 一级菜单栏(新,恒驻)
│ [后退][前进][刷新][首页] [====== 地址栏 ======]        │ ← 工具栏(既有)
├────────────────────────────────────────────────────────┤
│                    页面画面(页面区)                    │ ← 高度 = 内容区 - 三层
├────────────────────────────────────────────────────────┤
│ [信息][警告][错误] [搜索…] [✓Preserve] [清空] [✕]     │ ← Console 过滤行(面板开时)
│ ❯ 2026-01-01 map init ok                               │ ← 日志区(滚动,分色)
│ ⚠ tile fallback                                       │
│ ❌ Uncaught TypeError: …                               │
│ [> 输入 JS 表达式,回车执行                       ]    │ ← eval 输入行
└────────────────────────────────────────────────────────┘
```

- **视口语义**:面板打开/关闭 → 页面区高度变化 → 查看器沿用既有 `resize`
  命令上报新视口(`sync_content_size` 的 chrome 高度改为
  `MENU_H + TOOLBAR_H + console_open ? CONSOLE_H : 0`),宿主重排重渲染,
  新帧 1:1 铺满页面区。与人工拖动窗口**同一通道**,零新协议。
- **日志区分色**:log/info/debug/dir/table/count/time/group = 默认色;
  warn = 橙;error/assert = 红;input = `❯` 前缀加粗;result = `←` 前缀蓝色;
  eval_error = 红。级别过滤三开关(信息/警告/错误)+ 关键词搜索为查看器本地
  过滤(渲染层),不回流宿主。
- **macOS 实现**(`view/macos/`):NSButton 菜单行 + NSScrollView+NSTextView 日志
  (可滚动/可选中复制)+ NSTextField eval 输入(回车发送)+ NSButton 过滤行;
  面板开合强制重报视口(复用 `apply_zoom` 的置零重报技巧)。
- **Windows 实现**(`view/windows.rs`):BUTTON 菜单行 + 自绘日志区(WM_PAINT,
  等宽字体、滚轮滚动)+ EDIT eval 输入 + BUTTON 过滤行。
- **Linux**:查看器本就是 stdin 消费桩(无窗口),菜单栏不适用,如实保持现状。

### 单文件 1800 行拆分

`view/macos.rs` 已 1635 行且本轮 +400 行以上(高速增长预防性拆分要求),
按仓库拆分规范转目录:`view/macos/mod.rs`(入口/窗口装配/帧槽/定时器)+
`view/macos/chrome.rs`(工具栏/地址栏/缩放/主菜单)+ `view/macos/input.rs`
(人类键鼠/右键菜单/光标)+ `view/macos/devtools.rs`(一级菜单栏 + Console 面板,
新增代码)。逐行机械搬移零改写,外部路径零改动。

### 平台无关共享层(view/devtools_common.rs)

面板状态/快照合并/过滤判定/配色映射的**唯一真源**放 `view::devtools_common`
(`PANEL`/`CONSOLE_BUF`/`SNAP`/`FILTER`/`DEVTOOL_SLOT` 五本账 + `merge_snapshot`
/`entry_visible`/`json_string`/`toggle_panel_state`/`take_slot_and_merge`),
macOS 与 Windows 查看器共用,各自只做 UI 装配/绘制/消息循环——双平台语义
天然一致,单测只写一份。

### 测试钩子:LAEW_DEVTOOLS_AUTO

`LAEW_DEVTOOLS_AUTO=<panel>`(如 `console`)让查看器**启动即展开**指定面板,
走与人工点击完全相同的 `toggle_panel` 链路(视口重报 + `devtools` 命令 +
快照同步)。真机验证面板 UI 时,自动化环境无辅助功能权限、无法合成点击,
经此钩子完成像素级核验;未设置时零行为。

### 顺带修复:eval 异常文本缺错误消息

`__lsmEvalUser` 的 catch 原取 `String(e.stack)`(Boa 的 stack 不含错误消息行),
Console 面板里人工 `throw new Error('boom')` 只见堆栈不见 `boom`。改走
`__lsmErrText`(name + message + stack 全量),`eval_js` 工具的错误信封同步受益。

## 明确边界(如实上报,不装全)

- `Elements/Sources/Network/Performance/Memory/Application/Security/Lighthouse/
  Recorder` 九个面板仅按钮占位,点击显示「尚未实现」占位;
- `$1-$4`(历史选中元素,依赖 Elements 面板)未实现(`$0` 用 `tab.focused`);
- 点击报错跳 Sources 面板(Sources 未实现)、Live Expression、`console.table`
  的图形化表格(以文本表格近似)未实现;
- `Preserve log` 以「快照合并近似」实现:开启时换页/清空不丢历史,
  同页增长追加尾巴;与 Chrome 的逐条保留语义等价但非同一实现;
- 未捕获 Promise rejection 的独立捕获(现有 try/catch 链路已覆盖回调/定时器/
  监听器异常,微任务 rejection 属后续阶段)。

## 测试与验证

1. **单元测试**:`view/mod.rs` 命令解析(新命令/非法行)、帧 kind 常量互斥;
   `js_preprocess::wrap_console_eval` 包装(快捷语法/引号转义/语句);
   `push_console_logs`(`__clear` 标记/400 上限);PRELUDE console API
   (table/count/time/group)在 `script/tests.rs` 补用例;macOS 选择器存在性
   回归(新增 NSButton/NSTextView/NSAlert 选择器)。
2. **集成测试**:`tests/web_use_engine.rs` 风格(mock HTTP)补 console_eval
   端到端(表达式执行 → 台账回写 → 快照字段)。
3. **真机验证(macOS)**:`LAEW_CHROME_MODE` 无关;`laew -p` 提示词驱动
   `MCP_Web_LsmAgentBrowser_Use` 打开 `https://map.baidu.com/`,
   headed 查看器出现一级菜单栏;点 Console 出面板;`console.log` 类页面日志
   分级可见;eval 输入回显;像素采样核对页面区未被遮挡(截图取证用
   ImageMagick 像素统计,不以肉眼/AI 读图定论)。
4. **Windows**:交叉 `cargo check --target`(windows-rs 模块 macOS 零编译,
   必须另平台验证)+ 冒烟测试(`#[ignore]`)。
