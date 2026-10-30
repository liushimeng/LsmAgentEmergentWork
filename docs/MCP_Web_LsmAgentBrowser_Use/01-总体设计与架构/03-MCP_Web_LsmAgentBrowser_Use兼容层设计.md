# MCP_Web_LsmAgentBrowser_Use 兼容层设计（LsmAgentBrowser 后端）

## 一、目标

`MCP_Web_LsmAgentBrowser_Use` 的**工具名、action、参数名、响应信封、错误码语义保持不变**。Agent 提示词、
历史会话、编排器（QC 机械足迹、SubAgent 页面复用提示）无需改动；只有底层从 Chrome/CDP
换成进程内 `LsmAgentBrowser`。

实现分两层：

```
src/agent/tools/mcp_web_use/     兼容层:参数归一化 / 校验 / 信封 / sequence / explore
        ↓
src/agent/browser/               内核:窗口/标签页 / 网络 / DOM / 布局 / 渲染 / JS
```

## 二、响应信封

```json
{"code": 0, "message": "ok", "data": {}}
```

- `code` 为整数，`0` 成功；
- `message` 为字符串；
- `data` 恒为对象（可能为空）。

## 三、错误码

| 码 | 含义 | 新内核触发点 |
| --- | --- | --- |
| 0 | 成功 | — |
| 1001 | 参数错误 | 缺 action/url/page_id/selector、非法枚举、互斥参数、steps/queries 超限 |
| 2000 | page_id 无效/已关闭 | 标签页不存在 |
| 2001 | 页面断连/导航失败 | 网络错误、DNS/TLS/超时、SSRF 拦截 |
| 2002 | 动作失败 | 选择器未命中、元素不可见、JS 异常、等待超时、单步超时 |
| 2003 | 页面崩溃 | 保留（阶段一未触发） |
| 3001 | 环境缺失 | 渲染缓冲区创建失败 |
| 3002 | 能力不可用 | `reuse_existing` / `connect_url`（无外部浏览器可接管） |
| 4001 | 人工介入不可用/超时 | HITL 无通道或等待超时 |
| 4002 | 人工取消任务 | 人工在弹窗/TUI 取消；同时置位进程级取消旗标 |
| 4003 | 人工介入进行中 | 保留（阶段一同一时刻只有一个 HITL 槽位） |
| 6001 | 任务锚点越界 | `open/navigate/new_tab/new_window` 目标站外 |
| 6002 | 人工核验输入闸 | 保留（阶段一以 `inspect(blockers)` 文本证据提示） |
| 6003 | 后台标签页限制 | 保留（阶段一所有页面均可读） |
| 6004 | 站点节奏冷却 | 保留（新内核不做 Chrome 身份频控） |
| 4999 | 任务关闭导致 sequence 中断 | 保留 |

## 四、顶层 action

| action | 参数 | 行为 |
| --- | --- | --- |
| `open` | `url*`、`mode`、`reuse`、`new_window`、`window_width/height`、`timeout_ms`、`user_agent`；兼容接受 `guard/overlay/stealth/highlight/wait_until/block_resources` | 打开页面，返回 `page_id/title/final_url/reused/mode/...`;站点依赖 WebGL 时附 `capability_gaps`(见 `26-` §四) |
| `list` | — | `{pages:[...], count, windows}` |
| `close` | `page_id*`（可 `all`/`background`/`others`）、`keep`（others 必填） | 关闭页面/窗口 |
| `control` | `page_id*`、`control_action*`、`params` | 写操作 |
| `inspect` | `page_id*`、`info*`、`params` | 观测 |
| `sequence` / `batch` | `steps*`（1..24）、`stop_on_error` | 连续步骤 |
| `explore` | `queries*`（1..8）、`page_id` 或 `url`、`summary_hint` | 批量观测 |

### open 返回字段

`page_id`、`title`、`final_url`、`request_url`、`reused`、`mode`、`mode_hint`、
`browser_reused`、`connect_mode:false`、`window_id`、`status`、`content_type`、
`byte_size`、`redirect_chain`、`highlight`、`guard`、`overlay`、`window`、`next_steps`；
命中重复访问时附 `visit_note`；落地空白页附 `page_health`。

> 阶段一 `mode` 恒为 `hidden`（进程内无头内核）。请求 `headed` 时附 `mode_hint` 说明原生窗口在阶段三实现。

### close 语义

- `close(page_id="all")`：清空全部标签页与窗口（仅内存，不杀任何系统进程）；
- `close(page_id="background")`：关闭非前台标签页；
- `close(page_id="others", keep=...)`：保留 `keep`，其余关闭；
- 具体 id：不存在返回 2000。

## 五、control_action 映射

| control_action | 参数 | 阶段一实现 |
| --- | --- | --- |
| `click` / `human_click` | `selector*`、`nth` | `<a href>` 导航；`target=_blank` 新标签页；submit/image 才提交表单；复选框切换值；点击 select 展开面板、点击 option 选中并收起；disabled 目标返回 `clicked:false + disabled:true` |
| `right_click` / `double_click` | `selector*`、`nth` | 返回元素坐标与动作类型，不导航 |
| `hover` / `mouse_move` | `selector*`/`x,y` | 记录指针位置；selector 模式同时命中目标与祖先链的 `:hover` |
| `input_text` / `human_input` | `selector*`、`text*` | 写入运行时值；`value_after` 返回长度/前缀；disabled/readonly 目标返回 2002 |
| `clear_input` | `selector*` | 清空运行时值；disabled/readonly 目标返回 2002 |
| `select_option` | `selector*`、`value/values/text/index` | 原生 `<select>` 选中并自动收起浮层面板 |
| `select_dropdown` | `option_text*`、`selector` | 按选项文本匹配原生下拉 |
| `key_press` | `key*`、`selector`、`modifiers` | `Enter` 提交所在表单；其余按键记录 |
| `press_sequence` | `keys*`、`selector` | 顺序处理按键（含 Enter 提交） |
| `scroll` | `selector` 或 `delta_x/delta_y` | 更新滚动偏移 |
| `scroll_to` / `scroll_to_text` | `selector*` / `text*` | 定位元素/文本并更新滚动 |
| `navigate` | `url*`、`timeout_ms` | 导航并返回 `final_url/title/status` |
| `back` / `forward` / `reload` | — | 历史栈操作 |
| `new_tab` | `url*` | 在来源窗口新开标签页，返回 `spawned_page_id` |
| `new_window` | `url*` | 新建逻辑窗口，返回 `page_id/window_id` |
| `close_tab` | — | 关闭当前标签页 |
| `bring_to_front` | — | 设为前台标签页 |
| `wait` | `selector`+`state`+`timeout_ms` 或 `duration_ms/ms` | 元素轮询或纯等待 |
| `eval_js` | `expression*`（兼容 `script/function/js/code`）、`await_promise`、`timeout_ms` | Boa 执行 + DOM 快照桥；返回 `result/value_type/console/actions_applied` |
| `set_cookie` / `delete_cookie` | `name*`、`value`、`domain`、`path`、`url` | 全内存 Cookie Jar |
| `set_storage` / `clear_storage` | `kind=local/session`、`items`/`key+value`、`keys` | 全内存存储 |
| `set_viewport` | `width*`、`height*` | 调整视口并重排 |
| `screenshot` | `save_path`（兼容别名 `output_path`/`path`）、`full_page`、`format`、`ocr`、`return_base64` | 软件渲染 PNG；`ocr=true` 返回布局可见文本并标注 `ocr_source` |
| `download` | `url` 或 `selector`、`save_dir`、`filename` | HTTP 下载落盘 |
| `heartbeat` | — | 返回标题 |
| `focus` / `blur` | `selector*` | 记录焦点选择器并同步 `:focus`/`:focus-visible` 运行时状态 |
| `dispatch_event` | `selector*`、`event*` | click/submit 走真实语义；其余记录 |
| `upload_file` | `selector*`、`file_paths`/`file_path` | 记录文件路径到表单值并立即重排，文件名直接进入布局/截图（阶段四读取文件） |
| `drag` | 源/目标 selector 或坐标 | 记录拖拽轨迹 |
| `set_window` / `sync_viewport` | 宽高/位置 | 逻辑视口调整；原生窗口阶段三 |
| `set_highlight` / `set_overlay` / `set_guard` / `auto_follow` | 兼容参数 | 记录状态，返回 `supported:false` + 说明 |
| `set_stealth` | `mode` | 返回 `applied:false`：原生内核对页面零注入，无伪装需求 |
| `request_human` | `reason`、`message`、`options`、`multi_select`、`timeout_ms`、`image_path` | 桥接 `human_assist`（桌面弹窗/TUI），映射 0/4001/4002 |

## 六、inspect info 映射

| info | 返回 |
| --- | --- |
| `console` | `{events:[{level,text,at_ms}], count, collection_healthy, last_event_at}` |
| `network` | `{events:[{url,method,status,content_type,bytes,elapsed_ms,redirect,at_ms}], count, ...}` |
| `elements` | `{ok,tag,selector,id,classes,text,value,state,href,attributes,visible,rect{...},count,index}`；`state` 归一化 checked/selected/disabled/readonly/indeterminate/open/select_open/focused/hovered/active；`rect.laid_out` 区分"没有布局盒"与"零尺寸盒"，被祖先 `display:none` 剪枝时附 `render_blocked_by`；`params.include_style=true` 时附加 `computed_style:{...}` |
| `stylesheets` | `{ok,count,rules,sheets:[{index,href,media,bytes,rules}],media:{width,height,prefers_dark,print}}` |
| `dom` | `{ok,selector,node_count,tree}` 或 `{ok,selector,outer_html}` |
| `localstorage` / `sessionstorage` | `{key: value}`（支持 `prefix`/`contains` 过滤） |
| `cookies` | `{cookies:[{name,value,domain,path,expires,httpOnly,secure,sameSite}]}` |
| `screenshot` / `ocr` | 与 control screenshot 相同；`ocr` 返回 `ocr_text/ocr_block_count` |
| `page_meta` | `{page_id,url,title,viewport,user_agent,status,content_type}` |
| `viewport` | `{width,height,devicePixelRatio,scrollX,scrollY,scrollWidth,scrollHeight,overflow,next_action}` |
| `url` / `title` | `{url}` / `{title}` |
| `ping` | `{ok,page_id,latency_ms,ts}` |
| `image_urls` | `{img_count,img_urls,data_image_count,canvas_count,svg_count,picture_count}` |
| `blockers` | `{blockers[],blocked,suggested_action,available_reasons}`（文本关键词证据） |
| `visibility` | `{visibility,focused,hidden,note}` |
| `extract_links` | `{links:[{href,text,context,is_external}],total,truncated,scanned,hostname}` |
| `extract` | `{items,total,returned,scanned,truncated,field_names,hostname,hint}`；`probe=true` 返回候选类名 |
| `page_state` | `{found,jsonld,globals,window_globals,...}`（阶段一读取 JSON-LD） |
| `coverage` | 进程级访问台账（与旧契约一致，仍要求 `page_id` 语法） |
| `text` | 页面可见文本：`{text,source,chars,returned_chars,truncated}`。`source=layout` 与截图内容一致；`source=dom` 表示布局层没有可见文本（被剪枝或由页面脚本生成） |
| `page_health` | 页面健康度：`{dom_nodes,layout_boxes,visible_boxes,layout_lines,visible_text_chars,page_height,console_events,console_errors,first_errors,blank_page,diagnosis,hint}`。把"渲染空白"翻译成可执行结论，Agent 据此改走非 JS 入口，而不是反复重试 |

## 七、page_id 约定

- 格式 `p_` + 8 位十六进制（如 `p_00ab12cd`）；
- `open` 获取，`control/inspect` 复用；`new_tab`/点击 `_blank` 返回 `spawned_page_id`；
- sequence/batch 支持 `$page_id`、`${page_id}`、`$spawned_page_id`、`${spawned_page_id}`；
- 步内 `page_id` 继承顺序：step > 顶层 page_id > 当前/派生页；
- `close` 支持 `all` / `background` / `others` + `keep`。

## 八、sequence / batch / explore

- `sequence` 与 `batch` 语义一致：`steps` 1..24；缺 `action` 时按 `control_action`/`info` 推断；
- 禁止嵌套 sequence/batch；
- 单步 150s 超时（`request_human` 豁免）；
- 默认 `stop_on_error=true`，失败即停；`false` 时继续并返回完整报告；
- 外层 `data`：`{mode:"continuous", steps, step_count, completed, stop_on_error, page_id, failed_at?}`；
- `explore`：1..8 个 `{info, params}`；无 `page_id` 且有 `url` 时自动 `open`（`data.auto_opened=true`）。

## 九、兼容参数处理原则

1. **路径类参数接受常见别名**（不再静默忽略）：`screenshot` 的落盘路径接受
   `save_path`/`output_path`/`path` 三种写法 —— LLM 常按直觉写别名，静默忽略会让
   Agent 找不到自己要的截图。
2. **接受但不生效**的参数返回结构化说明，不返回 1001：
   - `reuse_existing` / `connect_url` / `auto_relaunch` → **3002**（无外部浏览器可接管）；
   - `stealth` / `set_stealth` → `applied:false`；
   - `guard` / `overlay` / `set_guard` / `highlight` / `auto_follow` → 记录状态，`supported:false`；
   - `block_resources` / `execution_mode` → 接受并忽略。
3. **已移除的 Chrome 身份治理**（频控冷却 / 整页抹除总账 / 反检测注入）不再产生 6002/6003/6004；
   这些码保留在契约中，避免历史提示词/测试断裂。
4. 任务锚点 6001 继续生效，接入点与旧实现一致（显式 URL 动作）。

## 十、阶段二/三的扩展点

- 页面脚本执行：`Document` → 持久 DOM + 事件循环；`JsRuntime` 换 V8（✅ 第一步/第二步已落地）；
- 样式与资源：CSS 级联引擎、外链样式预取内联、`getComputedStyle`（✅ 阶段二第二步）；
  图片/字体等子资源、缓存、下载管理（⏳ 阶段四）；
- 原生窗口：`BrowserMode::Headed` 不再回退，`guard/highlight` 产生真实视觉效果；
- 反爬治理如仍有需要，改为**内核原生请求策略**（限速/重试/Cookie 隔离），不恢复 CDP 注入。
