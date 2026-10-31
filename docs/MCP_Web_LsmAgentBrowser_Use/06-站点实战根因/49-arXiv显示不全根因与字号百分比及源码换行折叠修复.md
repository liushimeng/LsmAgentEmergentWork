# 49 - arXiv「显示不全、布局混乱」三根因修复(2026-10-31 实测)

- 时间:2026-10-31
- 场景:`MCP_Web_LsmAgentBrowser_Use_arxiv_01` 真机跑「arXiv 找 AI Agent 论文」
  + `examples/lsm_probe` 探针(1920×1080,hidden)+ `LAEW_DUMP_HTML` /
  `LAEW_DUMP_DISPLAY` + 全页截图 + `eval_js` 祖先链实测。
- 现象:首页全页截图 **7680×1082**(视口 1920);hero 文案逐词断行、字号 200px;
  主题分类列表逐行**阶梯状右移**。
- 结论:三个**通用层**根因叠加,与 arXiv 站点本身无关:
  1. `font-size` 百分比按**视口宽**求解(应相对父字号);
  2. 合并冲突丢失「源码换行折叠」,`<li>` 文本节点里的缩进换行被当**硬换行**;
  3. 换行后行首仍取**累计 pen**(应回到容器内容左缘)—— 折叠修完仍阶梯。
- 提交:通用层修复(`style/values.rs` + `layout/inline.rs`),零站点特判。

## 1. 根因一:百分比字号按视口宽求解

arXiv 全局样式:

```css
#content, #content-inner { margin: .7em; font-size: 90%; }
```

内核 `style/values.rs::parse_font_size` 对百分比没有单独分支,直接落到
`parse_length`;而 `parse_length` 的 `%` 分支是:

```rust
if let Some(n) = v.strip_suffix('%') {
    return n.trim().parse::<f64>().ok().map(|n| n / 100.0 * ctx.viewport_width);
}
```

即**所有 `%` 一律按视口宽近似**(该近似对 `width`/`margin`/`padding` 等
「相对包含块」属性是既定设计)。于是:

```
font-size: 90%  →  0.9 × 1920 = 1728px  →  font-size 侧 clamp(6,200)  →  200px
```

`#content` 变 200px,其下所有**未显式声明字号**的继承者(hero 文案、主题列表、
页脚)全部炸成超大字;200px 字号下 `distribution` 一词宽 ~1200px,超出内容宽 →
逐词断行;`margin:.7em` / `padding:.3em` 也跟着 ×200 放大。

**实测**:`inspect(elements, .tagline)` → `font-size: 200px`、`line-height: 290px`、
`padding-bottom: 60px`;`eval_js` 走祖先链:

```
html 16 → body#front 16 → div.flex-wrap-footer 16 → main 16
  → div#content 200   ← 源头
    → div.columns 200 → div.column.intro-and-news 200 → p.tagline 200
```

### 修复

`parse_font_size` 在 keyword 之后、`parse_length` 之前补 `%` 分支(相对
`parent_size`,符合 CSS 规范):

```rust
if let Some(n) = v.strip_suffix('%') {
    return n.trim().parse::<f64>().ok().map(|n| parent_size * n / 100.0);
}
```

影响面仅 `font-size` 的百分比解析;`em`/`rem`/绝对长度走原路径不变;
`line-height` 的 `%` 已在 `parse_line_height` 里按字号处理。

## 2. 根因二:合并冲突丢失「源码换行折叠」+ 行首取累计 pen(阶梯右移)

修完字号后文字可读,但**整页宽度仍 7473px**:主题列表每个 `<li>` 的行首 x 依次
`30.1 → 117 → 250.3 → 326.7 → 417`,每行起点 ≈ 上一行终点。

### 根因链

```
HTML 源码缩进换行(<li>\n  <a>…</a>\n  (<strong>…)
  └→ 文本节点 "\n      (" 含非空白字符 → collect_inline_children 不跳过
       └→ push_styled_decor 原样收 run(未折叠)
            └→ flush_run 把 '\n' 当硬换行 → commit_line + reset
                 └→ 下一行 line.origin_x = run.x(累计 pen,非容器左缘)
                      └→ 行首逐行右移 → 阶梯 → 整段溢出容器右缘(7473px)
```

`git log -S "fn collapse_ws" -- src/agent/browser/layout/inline.rs` 显示:
`collapse_ws` 与其在 `push_styled_decor` 里的调用,在 `3717222`(京东页脚
5777px 的修复)里**存在**;`9340641` 合并 `origin/main` 时**保留了 `preserve`
字段却丢了 `collapse_ws(text)` 调用**,折叠逻辑整体消失 —— 京东页脚那次
「源码换行当硬换行」的修复被静默回退了(典型「合并冲突解决不彻底」回归)。

### 修复

恢复 `inline.rs::collapse_ws`(ASCII 空白 → 单空格,NBSP 不折叠)与
`push_styled_decor` 里的调用:

```rust
let text = if preserve { text.to_string() } else { collapse_ws(text) };
```

### 根因二·补:**换行后行首仍取累计 pen**(折叠修完仍阶梯)

折叠修完,`\n` 不再产生假换行,但**正常换行**(内容超容器宽)依旧阶梯:
每行起点 = 上一行终点。根因在 `flush_run` 的行首取值:

```rust
if !line.started {
    line.origin_x = run.x;   // ← run.x 是「不换行」时的累计 pen
    ...
}
seg_x = line.flow_x.max(run.x);
```

`run.x` 是收集期的**累计位置**(pen 累加,只被 `line_x + avail_w` 截断)。
换行后 `line.reset()` 把 `flow_x` 归零,但 `run.x` 仍是累计值 → 行首取到
累计 pen → 每行起点逐行右移。

**修复**:`flush_inline_floor` 以「缓冲里首个非空 run 的 x」为**容器内容左缘**
`base_x`;`flush_run` 行首一律回到 `base_x`,并用
`rel = base_x + (run.x - line.first_run_x)` 求**行内相对位置**
(`first_run_x` = 本行首个 run 的 x):

```rust
if !line.started {
    line.origin_x = base_x;
    line.flow_x = base_x;
    line.first_run_x = run.x;
    line.started = true;
}
let rel = line.origin_x + (run.x - line.first_run_x);
seg_x = line.flow_x.max(rel);
```

这与 CSS 一致:**跨行内联元素的续行回到包含块内容左缘**(首行片段才带元素
自身位置)。旧行为(续行取 `run.x`)既阶梯又让续行凭空缩进。

## 3. 验证

- 新增单测:
  - `style/tests.rs::百分比字号相对父字号而非视口宽`;
  - `layout/inline_fragment_tests.rs::源码缩进换行折叠为空格_列表不阶梯右移`;
  - `layout/inline_fragment_tests.rs::连续空白折叠为单空格_不产生多余断行`;
  - `layout/inline_fragment_tests.rs::长行换行后行首回到容器左缘_不阶梯右移`。
- `cargo test --lib` 全量回归(2 条 `script::tests` 的 JS 线程超时失败为
  **既有环境抖动**,`git stash` 对照在 pristine HEAD 上同样失败)。
- 探针复跑:`.tagline` 字号 200 → **14.4**;全页截图宽度 7680 → **≈1920**
  (横向溢出消失);主题列表回到多词一行的正常排版。

## 4. 排查方法沉淀

1. **`git log -S "<符号>" -- <路径>` 是「修复被合并回退」的照妖镜**:
   `fn collapse_ws` 在 `3717222` 存在、`HEAD` 不存在,一搜即出。
   合并冲突里「保留了新字段、丢了配套的调用」是最易漏的形态。
2. **行首 x 阶梯右移**(每行起点 ≈ 上一行终点)是「源码换行被当硬换行 +
   行首取累计 pen」的**特征指纹**,一眼可辨。
3. **百分比语义要按属性分流**:`parse_length` 的「% 按视口宽」近似只对
   「相对包含块」属性成立;`font-size`/`line-height` 这类**相对字号**的属性
   必须单独分支(否则 90% × 1920 直接炸穿 `clamp` 上限)。
4. **`clamp` 会掩盖量级错误**:200px 不是「字号 200」而是「1728 被 clamp」——
   看到 clamp 边界值要反查输入。

## 5. 边界与后续

- 仍不解析 `@font-face`(arXiv 的 IBM Plex / STIX 全部 404)、无 SVG 光栅化:
  属既有能力边界,回退系统字体 / 跳过图标。
- `header.js` 抛 `TypeError: not a callable function at fold`:设计系统头部脚本
  一处 Boa 兼容缺口;静态 DOM 已渲染,不影响主体取证,单独排查。
