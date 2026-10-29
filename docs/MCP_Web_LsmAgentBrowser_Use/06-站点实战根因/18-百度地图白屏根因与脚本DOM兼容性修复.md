# 18 - 百度地图白屏根因与脚本/DOM 兼容性修复

**轮次**:2026-10-22(百度地图路线规划任务实测驱动)

## 一、问题与定位方法

`https://map.baidu.com/` 在内核中只渲染出搜索框骨架,地图区域纯白;tangram 工厂首屏
抛 `TypeError: cannot convert 'null' or 'undefined' to object`,随后
messageCenter / userMgr / Storage 整层 `baidu is not defined`。

由于 Boa 0.20 的运行时错误不带源码位置,本轮补充了三层诊断能力(默认零开销):

1. `JsRequest::Diag` + `LAEW_JS_DIAG=1`:在持久 Context 上做语句级前缀重放二分;
2. `LAEW_JS_TRACE_STMT=1`:宿主给 `define(id, factory)` 工厂体插 `__lsmStmtMark`,
   工厂抛错时消息带 `last=<最后执行语句>`;
3. `LAEW_JS_TRACE_TOOBJ=1`:包装 `Object/Array/String/RegExp/Date` 内建,
   记录 null/undefined 的 ToObject 调用点。

同时修复 `diag.rs` 在多字节 UTF-8 字符中间切片导致的 panic(400KB 中文注释脚本必现)。

## 二、根因链(7 处通用缺陷)

### 1. 脚本执行顺序被打乱

`collect_scripts` 把内联脚本与外链 URL 收集成两个数组,管线先执行全部内联、再按序执行
外链。百度地图的初始化链是:

```
mod.js(安装 RequireJS/FIS) → 内联 require.resourceMap(...) → pano-init-pkg → ... → initmap
```

重排后 `resourceMap` 在 `mod.js` **之前**执行,配置落到了内核的最小 AMD 装载器上;

### 2. AMD 依赖补齐破坏 FIS 同步注册

`amd_fill_deps` 把所有无 deps 的 `define(id, factory)` 改写成
`define(id, ['require','exports','module',...], factory)`。FIS 的 `define` 只识别
两参形态,三参形态被转交 RequireJS 分支进入 `globalDefQueue`,同步注册表 `d` 永远为空
→ `PanoLoading.js has not been loaded yet`。

修复:**运行时按当前装载器形态逐脚本判决**。`__lsmAmdKind()` 在每次执行脚本前识别:

| 形态 | 判定 | 是否补齐 |
|------|------|----------|
| RequireJS | `require.s.contexts` 存在 | 是(保留 tangram `to_object` 修复) |
| FIS | `require.loadJs/loadCss/alias` 存在且无 `.s` | **否**(保持两参同步定义) |
| 内核最小装载器 | `require.defined` 存在 | 是 |

### 3. 交互后不执行懒加载脚本

`require.async` 通过 `document.createElement('script')` 注入模块,宿主在 open/navigate
时跑管线,交互后从不取回新脚本。路线面板等 widget 永远停在 loading。

修复:`apply_js_reply` 收口后调用 `run_pending_scripts`,复用 `tab.js` 与
`tab.executed_scripts` 续跑管线(8s / 12 轮),只执行未执行过的脚本。

### 4. Annex B `String.prototype.substr` 缺失

Boa 0.20 默认关闭 annex-b,站点自带补丁在本环境抛 ToObject,tangram 第一条
`createChain` 即中断。PRELUDE 按 Annex B.2.2.1 提供正确实现,并在 `__lsmRefresh` 与
每个脚本执行前重新断言(页面脚本可能覆盖成坏实现)。

### 5. detached innerHTML 没有可查询子树

tangram 特性探测:

```js
var div = document.createElement('div');
div.innerHTML = '<input type="checkbox"/>';
input = div.getElementsByTagName('input')[0];
input.checked = true;   // input 为 undefined → ToObject
```

修复:新增轻量解析器 `__parseHtml`,innerHTML 赋值时生成 `__htmlKids`;
读路径(`children/childNodes/first/last/getElementsByTagName/getElementsByClassName`)
同时覆盖解析子树与动态 `__kids`。序列化仍使用 `__pendingHtml` 原文,保持挂载保真。
`children` 按浏览器语义只返回元素节点。

### 6. 定位父元素下的百分比高度退化

布局把「定位元素」一律当成包含块高度未知:

```css
#app { position: relative; height: 720px }
#MapHolder { height: 100% }   /* 实际只得到 8px(内容高) */
```

修复:`own_definite_h` 为真时,无论是否定位,子代包含块高 = 本盒内容高;仅 auto 高
使用 `UNKNOWN_CB_H`。

### 7. html/body 无布局盒 → body.clientWidth/Height = 0

布局从 body 内容开始,html/body 不进 `rect_map`,快照 rect 为 null。地图初始化直接读
`document.body.clientHeight`,得到 0。修复:`__lsmGeom` 对 html/body 按视口兜底。

## 三、附带兼容项

- `window.status/defaultStatus`(messageCenter 直接写裸 `status`);
- `canvas.getContext('2d')` 空实现:让 canvas 重度依赖页面完成初始化(不绘制像素);
- AMD `define` 归属包装:工厂抛错时消息带模块 id 与最后语句标记;
- 定时器错误带注册来源(脚本 URL + 语句标记);
- `TabState.executed_scripts` 与 `PipelineSeed`:交互续跑与首屏共用一本执行账。

## 四、修复后的实测状态

| 指标 | 修复前 | 修复后 |
|------|--------|--------|
| `typeof T` / `typeof baidu` | `undefined` | `function` |
| 首屏 tangram 错误 | ToObject,整层失败 | 无 |
| `#MapHolder` 高度 | 8px | 720px |
| `document.body.clientWidth/Height` | 0 / 0 | 1280 / 720 |
| 依赖 tangram 的模块 | 4+ 个 ReferenceError | 全部正常加载 |
| 路线按钮 | 点击无变化 | 进入页面 JS 处理器 |

## 五、剩余缺口

1. **canvas/WebGL 像素渲染**:地图瓦片、矢量底图、路线绘制均在 canvas;当前只有空 2D
   上下文,页面能初始化但画不出地图。这是「白屏」的最后一段,需要独立实现光栅化/GL
   管线,或找到站点的 DOM/SVG 渲染分支。
2. **地图就绪事件依赖瓦片**:底图未绘制时站点不会移除 `loading-button`,路线面板不会
   挂载;DOM 交互链路虽已打通,面板仍不出现。
3. 风控脚本 `abclite-*.js` 与活动 JSONP 接口在本环境不可用,属站点侧限制。

## 六、调试开关

| 开关 | 作用 |
|------|------|
| `LAEW_AMD_FILL=off` | 关闭 AMD 依赖数组补齐 |
| `LAEW_JS_TRACE_STMT=1` | define 工厂体语句标记 |
| `LAEW_JS_TRACE_TOOBJ=1` | ToObject 调用点追踪 |
| `LAEW_JS_DIAG=1` | 语句级错误定位(含 UTF-8 越界修复) |
