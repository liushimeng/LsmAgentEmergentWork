# 21 · Sizzle 选择器引擎与 `document.nodeType` 修复（整页 widget 不渲染根因）

- 日期：2026-10-23
- 状态：已完成
- 关联：`../06-站点实战根因/18-百度地图白屏根因与脚本DOM兼容性修复.md`、`../05-窗口交互与页面观察/20-浏览器窗口交互化与地图页白屏根因.md`

## 1. 现象

有头窗口能拉起，但窗口里只有左上角一个搜索框 + 一个按钮，其余全白。
`inspect info=page_health`：`dom_nodes=128`、`layout_lines=0`、`visible_text_chars=0`；
`inspect info=text` 只有「百度地图」4 字符；截图 28KB（近乎空白）。

## 2. 根因

百度地图的控件/面板全部走 **Tangram（`baidu`/`$`/`T`）** 这套 jQuery 风格封装，选择器入口是 Sizzle：

```js
function (selector, context, results) {
    results = results || [];
    context = context || document;
    var nodeType = context.nodeType;
    if (nodeType !== 1 && nodeType !== 9) { return []; }   // ← 关键守卫
    ...
}
```

内核快照桥的 `document` 对象**没有 `nodeType` 字段**（`undefined`），于是 Sizzle 对任何选择器都直接 `return []`：

- `baidu('#mapType')` / `baidu('#sole-input')` / `baidu('div')` 全部返回空集；
- `mapTypeTab` / `messageCenter` / `NavigationControl` / `ScaleControl` 等 widget 静默不渲染；
- 页面只剩服务端直出的空壳。

**为什么难以察觉**：内核自带的 `document.querySelectorAll`（`__lsmQuery`）一直是好的
（`#mapType` 命中 1），所以 `inspect info=elements` 看起来完全正常；只有走 jQuery/Sizzle
的页面脚本会中招。

运行时对照（`eval_js` 实测）：

| 表达式 | 修复前 | 修复后 |
| --- | --- | --- |
| `document.nodeType` | `undefined` | `9` |
| `baidu('div').length` | `0` | `42` |
| `baidu('#mapType').length` | `0` | `1` |
| `document.getElementsByTagName('*').length` | `120` | `141` |

## 3. 修复

给两处快照桥的 `document` 补 `nodeType: 9`（DOM 规范 `Node.DOCUMENT_NODE`）：

- `src/agent/browser/js.rs`：`PRELUDE_BODY` 的 `var document = {`（阶段一回退快照桥）
- `src/agent/browser/script/prelude.rs`：PRELUDE v2 的 `var document = {`（阶段二持久 JS 线程）

两处缺一不可。

## 4. 同批修复：内联样式 camelCase 未归一化

站点经 JS `el.style.backgroundImage = ...` 写内联样式，序列化回 HTML 后属性名保留
camelCase（`backgroundImage` / `textAlign` / `zIndex` / `MSTouchAction`）。
样式引擎 `parse_declarations` 只做了 `to_ascii_lowercase()`，`backgroundimage` 匹配不到
`background-image`，背景图/对齐/层叠全丢。

新增 `normalize_property_key` / `kebab_case`（`src/agent/browser/style/mod.rs`）：

| 输入 | 输出 |
| --- | --- |
| `backgroundImage` | `background-image` |
| `textAlign` | `text-align` |
| `zIndex` | `z-index` |
| `MSTouchAction` | `-ms-touch-action` |
| `WebkitTransition` | `-webkit-transition` |
| `--MainColor` | `--maincolor`（自定义属性仅小写化） |

## 5. 同批修复：`LAEW_BROWSER_MODE` 优先于工具 `mode` 参数

实测 LLM 会自行选 `mode=hidden`，导致用户看不到有头窗口。
新增 `BrowserMode::from_env_or_arg`（`src/agent/browser/mod.rs`）：显式环境变量压过工具参数，
未设环境变量时仍以工具参数为准。

## 6. 验证

- 单测：`browser::script` / `browser::style` / `browser::` 全绿；`cargo test --lib` 2379 passed。
- 真实站点探针：`baidu('div')` 0→42、`baidu('#mapType')` 0→1、截图 28KB→305KB（横幅图片正常渲染）。
- 集成测试：`protocol_wire` / `project_context` / `web_use_engine` 全绿。

## 7. 已知边界（不因本次修复改变）

1. 地图瓦片（canvas/WebGL）不渲染，`#MapHolder` 区域仍空白 —— 需真浏览器（`MCP_Web_Chrome_Use`）兜底。
2. `#MapHolder{height:100%}` 在父高 auto 时按 CSS 语义退化成内容高（地图容器塌缩）。
3. `window.getSelection` / `document.createRange` / `ActiveXObject` 未提供，文本选择类交互不可用。
4. 百度风控/登录：未登录态复杂功能受限；按取证纪律如实报告，不绕过。

## 8. 排查手法沉淀（供后续同类问题复用）

「页面只剩空壳」类问题，按以下顺序剥离：

1. `LAEW_DOM_TRACE=1` 看动作是否落进真实 DOM（`@created:` 动作被静默忽略 = 游离元素丢失）。
2. `eval_js` 探 `document.nodeType` / `typeof baidu` / `baidu('div').length` —— **jQuery 风格站点的选择器引擎是高频盲区**。
3. `inspect info=page_health` 的 `dom_nodes` / `visible_boxes` 前后对比。
4.  dump HTML（`LAEW_DUMP_HTML`）离线比对 raw 响应，确认是「服务端直出」还是「JS 没建」。
