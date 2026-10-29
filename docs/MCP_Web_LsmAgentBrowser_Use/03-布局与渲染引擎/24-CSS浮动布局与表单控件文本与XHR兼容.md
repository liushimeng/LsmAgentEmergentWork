# 24 - CSS 浮动布局、表单控件文本与 XHR 兼容修复

**轮次**:2026-10-26(百度地图公交路线规划任务实测驱动)
**方案文档**:`tmpPlan/20261026_101838_百度地图显示问题修复与浏览器功能优化方案.md`

## 一、实测症状(修复前)

有头查看器 + 无头探针(`tests/tmp_map_layout_probe.rs`,用完即删)双路取证,
`https://map.baidu.com/` 在本内核里「显示不全、布局混乱」:

| 症状 | 证据 |
|------|------|
| 搜索卡片纵向堆叠 | `#sole-input` (28,31) 与 `#search-button` (28,75) 上下排列,真实浏览器是同一行 |
| 输入框是无字灰盒 | `inspect text` 只剩 DOM 标题 4 字,`visible_text_chars=0`、`layout_lines=0` |
| 地图/路线面板建不出来 | `page_health` 4 条 JS 错误:`xhr rs: TypeError cannot convert 'null' or 'undefined' to object` ×2、AMD `dataCopyRight.js` 同类 ToObject |
| 路线按钮连续 1001 | 模型把 `dispatch_event` 的 `event` 传成对象 `{"bubbles":true}`,参数校验直接拒绝 |

## 二、根因链(全部代码级坐实)

```
R1 float/clear 只落在 ComputedStyle.extra,布局完全不读
   → 百度卡片(.clearfix 浮动布局)退回普通块流,纵向堆叠(通用缺陷,非站点特例)

R2 行内浮动与块级浮动若只各做一套,会在同一容器里互相重叠
   真实结构(探针祖先链 + computed style 确认):
     #searchbox(block,425)                       clearfix
     ├─ #searchbox-container(block,float:left)   auto 宽 → 应收缩为内容宽 368
     │   └─ #sole-searchbox-content(block,368)
     │       ├─ #sole-input(inline,float:left,329)
     │       └─ .route-button(block,float:left,39)
     └─ #search-button(inline,float:left,57)
   尺寸恒等式:329+39=368,368+57=425 —— 一整条精确浮动行

R3 auto 宽浮动容器若用「大宽度铺开布局再量盒宽」做 shrink-to-fit,
   auto 块级子级会撑满测量宽 → 永远量出容器宽 → 浮动容器独占整行

R4 表单控件只发射背景+边框,value/placeholder 文本零绘制 → 灰盒无字

R5 XHR 回调裸调用(无 Event 实参)、_respHeaders 从不赋值、responseURL 缺失
   → 站点 onreadystatechange 抛 ToObject,城市/版权数据 boot 链中断
```

## 三、实现

### 3.1 样式层(`style/values.rs`)

- `ComputedStyle` 新增 `float`/`clear` 字段(默认 `none`,非继承);
- `apply_declaration` 真正解析 `float:left/right/none`(含 `inline-start/end`)
  与 `clear:left/right/both`;`inherit`/`initial`/`revert` 同步接入;
- `to_map` 输出 `float`/`clear`,`inspect elements(include_style)` 可直接观测。

### 3.2 布局层(`layout/float.rs` 新增,`layout/mod.rs` 接线)

| 件 | 职责 |
|----|------|
| `FloatState` | 块容器内块级浮动台账:按 y 求可用水平带、`clear` 下沉、容器 auto 高收编 |
| `InlineRow` | 行内上下文的「行占用」台账:当前行 y、左向内容右缘、右浮动左缘、行高、内容底 |
| `float::place/finish` | 块级子盒进入布局前求带定位,收尾登记并恢复流式游标 |
| `layout_inline_float` | 行内浮动(`input/button/span` 等)排在当前行剩余带,不足换行 |
| `max_content_width` | shrink-to-fit 的 max-content 估算:不铺布局,沿 DOM 子树累加 |

**统一行占用账(关键设计)**:块级浮动 `finish` 时同步登记进 `InlineRow`,
块级浮动 `place` 时按 `InlineRow` 当前行占用收窄可用带;行内浮动也只读
`InlineRow`。因此「行内输入框 329 → 块级路线按钮 39 → 行内搜索按钮 57」
三种混排共用同一本账,不会同点重叠。

**auto 宽 shrink-to-fit**:`max_content_width` 沿 DOM 子树估算——块级子级
取最大,行内/浮动子级同一行累加,显式宽与 `img` 按自身尺寸。
`#searchbox-container` 由此得到 368 而不是 425。

**表单控件文本**:`collect_inline` 行内建盒处新增 `emit_form_control_text`
(独立方法,只发射文本显示项,不动几何与游标):
- `input` 文本类(text/search/url/tel/email/password/number):value 优先,
  空则 placeholder(UA 灰 #757575);password 按 `•` 掩码;
- `input[type=submit/button/reset]`:绘制 value;
- `textarea`:value > 文本内容 > placeholder;
- 盒内 3px 内缩、垂直居中、按可用宽截断(与真实输入框裁剪一致)。

### 3.3 XHR(`script/prelude.rs`)

- 新增 `__lsmXhrEvent(type, xhr)`:`{type,target,currentTarget,timeStamp,...}`;
- `onreadystatechange`/`onload`/`onerror`/`addEventListener` 监听器统一带事件实参;
- `_respHeaders = r.headers` 回填,`getResponseHeader` 不再恒 null;
  新增 `getAllResponseHeaders()`;补 `responseURL`;
- `responseType:"json"` 时 `response` 给解析后对象(空串/解析失败给 null)。

### 3.4 工具参数通用性(`browser/api.rs`)

`dispatch_event` 的 `event` 兼容三种形状:字符串 `"mousedown"`、
对象 `{"type":"mousedown","bubbles":true}`、`params.type` 兜底;
缺参报错文案直接给出正确形状。

## 四、验证(全部实测)

1. `cargo test --lib`:**2484+ 通过 / 0 失败**(含新增 11 个布局用例);
   `cargo test --test web_use_engine`:**21 通过 / 0 失败**(含 XHR 事件与响应头、
   `dispatch_event` 三种参数形状两个新集成用例)。
2. 新增关键单测:
   - `浮动_行内控件与行内块排成一行`(百度卡片同构 329+39=368、368+57=425);
   - `浮动_块级浮动容器收缩宽度并与行内浮动混排`(真实百度 DOM 结构);
   - 左右浮动同行/右浮动贴右/容器 auto 高收编/clear 换行/带宽不足换行/侧栏收窄;
   - placeholder 灰字、value 优先、密码掩码。
3. **真站前后对照**(同一探针、同一 URL):

| 指标 | 修复前 | 修复后 |
|------|--------|--------|
| `#search-button` 位置 | (28,75) 输入框下方 | **(396,47.4) 与内容盒同一行**,28+368=396 精确贴合 |
| `#searchbox` 高度 | 44(内容堆叠后曾异常) | 44 |
| 可见文本 | `source=dom` 仅标题 4 字 | `source=layout` 11 字「搜地点、查公交、找路线」 |
| console 错误 | 4 条(`xhr rs` ×2 + AMD + 其它) | 2 条,**`xhr rs` 全消** |
| 截图 | 灰盒 + 孤立蓝块 | 输入框带 placeholder,蓝色搜索按钮紧邻其右 |

## 五、已知边界与后续

1. **行盒不做文字环绕**:浮动激活期间的行内/块级非浮动内容,有剩余带则收窄、
   无带则下沉到浮动之下;已生成的行盒不回溯重排。
2. **行级与块级浮动已覆盖**;`img` 替换元素与 flex/grid 容器的浮动仍走原路径。
3. **容器 auto 高收编浮动底边是 clearfix 友好近似**(真实 CSS 需 BFC/clearfix),
   取舍:不收编会让父盒塌缩、后续内容与浮动重叠,比近似更难看。
4. **auto 宽浮动盒保留引擎既有 40px 下限**,与块级分支 `max(40.0)` 口径一致。
5. **地图底图/路线面板仍未渲染**:剩余 `AMD dataCopyRight.js` 的 ToObject
   (`LAEW_JS_TRACE_TOOBJ=1` 无命中,疑似 `in` 运算符或未包裹的内建入口)
   与 canvas/WebGL 像素渲染缺口(20 号文档已列),是下一轮的主要对象。
6. Agent 侧另案(Debug 报告 P0):WorkFlow 单元完整性终态校验、
   SessionContext 摘要产物存在性校验、工具链硬约束。详见方案文档「遗留」。
