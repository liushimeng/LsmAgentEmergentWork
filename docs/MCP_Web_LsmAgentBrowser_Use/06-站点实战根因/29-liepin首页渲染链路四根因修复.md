# 29 - 猎聘首页渲染链路四根因修复(width 百分比预解 / inline-flex 块级化 / svg 固有尺寸 / flex 条目 seq)

> 对应方案:`tmpPlan/20261027_191114_猎聘首页渲染链路四根因修复.md`
> 触发场景:猎聘首页(未登录)在 LsmAgentBrowser 中「零星几个文字、布局乱、显示不全」,视口截图大片空白,全页高度灌水至 6965px。

## 一、症状与误诊陷阱

- **症状**:视口内只有深蓝 banner 色块 + 一行标题;布局文本(inspect text,source=layout)却包含全部页面文字。
- **陷阱 1**:AI 图像分析对全页截图「脑补」出完整卡片网格——**必须用像素统计**(ImageMagick 逐段非白占比)戳破,实测全页 99% 区域空白。
- **陷阱 2**:「布局有文本、渲染无像素」的矛盾,真因是**布局几何本身已灌水**(元素被推到视口外/被巨图撑开),不是光栅化丢内容。
- **陷阱 3**:三连写 `display:-webkit-box;display:-ms-flexbox;display:flex` 的级联终值**没有 bug**(同特异性按源序,末值胜出)——先写单测再下结论,别修不存在的 bug。

## 二、四根因与修法(全部通用,非站点特判)

### 1. 样式层百分比宽度预解(values.rs)
`width:<pct>` 曾被 `parse_length` 按视口宽立即求成绝对值(与 `height` 分支多年前的修法不一致)。logo `width:100%` → `Some(1920)`,方图按比例推高 → 1920×1920 巨图撑爆卡片,全页高度灌水。
**修**:百分比宽度只进 `width_percent`,`st.width=None`;布局层按包含块求解——`replaced_size` 签名增加 `(avail_w, avail_h)`,块级分支原有,flex 条目主/交叉新增,`float/position` 调用点跟上。**max-content 测量语境(avail≤0 或 ≥哨兵)下百分比按 auto 退化**。

### 2. inline-flex 被归一成 flex(style/values.rs + layout/mod.rs)
行内级容器被块级化后,`is_block()` 判真、逐个独占一行(行业 tab 整行竖排)。
**修**:`normalize_display` 保留 `inline-flex`;前缀 `-webkit-inline-box`/`-ms-inline-flexbox` 等 → `inline-flex`;`-webkit-box`/`-ms-flexbox` → `flex`(只写前缀的规则不至于退化行内);`inline_box_worthy` 增列 `inline-flex`。

### 3. svg 无替换语义 + 测量语境 auto 宽撑满(layout/mod.rs)
`<svg>` 当普通块级,`display:block` 下 auto 宽在 flex 条目测量(`MAX_CONTENT_WIDTH=100_000` 哨兵)里撑满 10 万 px → 条目巨宽、wrap 后单条目一行、居中到负 x。
**修**:`is_replaced_tag`(img|svg)统一走替换元素分支;svg 固有尺寸 = width/height 属性 → viewBox → 300×150;块级分支在测量语境下 auto 宽 shrink-to-fit(子代布局后按 lines/boxes 右缘回缩)。

### 4. flex 条目 seq 晚于内容文字 → 白底盖字(flex.rs)
文字 item 的 seq 取 flush 时刻(条目测量期),条目自身背景的 seq 取 emit 时刻(更晚);`(z,seq,rank)` 终排后**白底卡后画,整体遮没自家文字**。
**修**:flex `Item` 增 `seq` 字段,**收集条目时**取先序 seq,`emit_item` 沿用——条目背景/边框 seq 恒早于内容文字,rank 通道序收尾。

## 三、诊断工具沉淀

- **`LAEW_DUMP_DISPLAY=<路径>`**(render_pixmap):显示列表全量落盘(kind/x/y/w/h/op/fixed/z/clip/text 摘要)。「布局有文本/渲染无像素」「元素去哪了」类矛盾一锤定音。
- 方法论:真站 CSS 原文 + DOM 结构 → `#[test]` 秒级复现(真站探针 200s/轮 → 单测 0.01s/轮);先单测锁定再动代码。

## 四、修复效果(真机复测)

| 指标 | 前 | 后 |
| ---- | ---- | ---- |
| 全页高度 | 6965px | 3477px |
| 视口截图字节 | 58915 | 102875 |
| logo 布局尺寸 | 1920×1920 | 24×24 |
| 行业 tab | 竖排 x=-49408 | 横排 x=254→893 |
| 渲染确定性 | A=B=C | A=B=C(未引入非确定) |

## 五、已知遗留

1. banner 登录框/搜索栏高度坍塌(h=1):百分比高度依赖链独立课题。
2. header(React 水合,z=1000)部分导航项坐标错位:动态节点回认几何坐标系问题。
3. PRELUDE `getComputedStyle` 返回空对象:JS 侧探测能力受损,待按常用属性集补齐。
4. 卡片 detail-top→labels 间距 199px(期望 ~40):内容高测量仍偏大。
