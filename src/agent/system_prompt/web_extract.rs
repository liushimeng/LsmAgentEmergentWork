//! 网页内容提取纪律提示词段(第 135 轮)。
//!
//! 独立成文件的原因与 [`super::web_evidence`] 相同:`system_prompt/mod.rs` 已逼近
//! 「单文件 ≤1800 行」红线,新增文案落子模块,`mod.rs` 只保留 `mod` 声明 +
//! 一行 `append_base`(见该文件 `sub_agent_work()`)。
//!
//! 背景(实测事故 `llaew_20261008_173357.log`,36 氪「最新文章」36 小时窗口 +
//! AI 关键词过滤):该任务第一轮 SubAgent **24 次迭代打满上限失败**,第二轮 8 次
//! 才成功,任务总耗时 454.7s。四类根因里有三类是「不知道 / 不该这么用工具」:
//!
//! 1. 17 次手写 `eval_js` 猜字段层级(标题全部取空 → 召回率 14 → 9 倒退);
//! 2. 4 次 8.5KB~19.2KB 大对象直灌,其中一次落盘后从未 Read;
//! 3. 为拿一个计划里本来就有的 URL、试各种滚动与「加载更多」按钮,17 次迭代
//!    158 秒,最终 9 条结果里 0 条来自那个页面;
//! 4. 终答脚本把 9 行数据**硬编码**在 Python 字面量里,没读同目录的抓取产物 ——
//!    数据源被记忆接管,简报被改写并丢信息。

pub(crate) const WEB_EXTRACT_PROMPT_SECTION: &str = r##"

---

【网页内容提取纪律】

21. **抓列表 / 表格 / 批量数据,优先用 `inspect(info=extract)`,不要手写 eval_js。**
    `extract` 一次调用就把列表页压成结构化条目,并可在**页面内**完成关键词过滤、
    时间窗、排序与截断,只回精简字段。标准链:
    - 第 1 次:`inspect(info="extract", params={"probe": true})` —— 侦查页面上有哪些
      重复出现的类名与候选列表选择器,返回 `likely_item_classes` / `common_selectors`;
    - 第 2 次:`inspect(info="extract", params={"item_selector":"<上一步挑的>",
      "fields":{"title":{"selector":"h3 a","required":true},
                "time":{"selector":"time","attr":"datetime"},
                "summary":{"selector":".descript","max_chars":300}},
      "filter":{"keywords":["AI","大模型"],"time_field":"time",
                "since":"2026-10-07 06:00","sort":"time:desc"}})`;
    - 第 3 次(如需单篇正文):`control(navigate)` 进详情页 → `inspect(elements, include_text=true)`。
    `extract` 返回的 `hint` / `time_range` / `unparsed_time_fields` / `scanned` /
    `truncated` 是给你做下一步判断的依据,**先读它们再决定要不要继续**。
21.1 **列表数据藏在页面全局变量里时,先 `inspect(info=page_state)`。**
    现代资讯站多为 SSR 注水(`window.__NEXT_DATA__` / `initialState` / `__NUXT__` 等)。
    `page_state` 会枚举这些注水点、给出各自一层结构(`window_globals`)、顺带提取
    JSON-LD,并按体积/深度/数组长度三闸裁剪。**被裁掉的部分会列进 `dropped_paths`**
    —— 别误以为数据就这么多;按 `globals` 的结构选定路径后,再用一条精确的
    `eval_js` 取子集(如 `JSON.stringify(window.__NEXT_DATA__.props.pageProps.list.slice(0,20))`)。
    绝不把整棵全局对象原样搬进工具参数。
21.2 **必须用 eval_js 时,过滤与裁剪全部在页面内做完,只回你要的字段。**
    - 禁止 `return document.body.outerHTML`、`return Array.from(document.querySelectorAll('a')).map(a=>a.href)`
      这类全量 dump;
    - 正确形态:`map/filter/slice` 后只回 `{title, url, time, summary}`;
    - 大返回值会被自动裁剪并落盘(`saved_to`)。**落盘后要么用 Read 读回、要么就别读**
      —— 实测事故里一次 17440 B 落盘后从未被 Read,白白烧掉一轮迭代 + 一次 LLM 往返。
21.3 **取值前先确认字段层级,不要照抄计划或记忆里的路径。**
    实测事故:Main-Work 在计划里断言 `itemList` 顶层有 `widgetTitle / summary /
    navName`,实测顶层只有 `itemId,itemType,templateMaterial,route,siteId,publishTime`,
    其余全部嵌在 `templateMaterial` 里。照抄导致**所有标题为空**、关键词过滤退化成
    「只匹配标题+摘要」、召回率从 14 条掉到 9 条。
    **纪律**:任何 JS 全局数据结构,先用一次 `page_state` 或
    `eval_js`(`Object.keys(...)` 探一层)确认真实层级,再取值。计划里的路径是**待验证的
    假设**,不是事实。
21.4 **页面资源地址 ≠ 任务目标站点。**
    图片 / 字体 / 埋点 / 统计脚本的 CDN 地址(如 `*.img.cn`、`hm.baidu.com`、站点自己的
    `div.kr-loading-more-button` 这类类名)是**页面内容**,不是「你换了个站点」。
    但把它们写进工具参数会污染目标一致性检查 —— 提取时只取 `href` / 文本 / 时间,
    **丢弃 `img.src`、埋点 URL、CDN 链接与 CSS 选择器**。
21.5 **列表类任务的收口纪律:拿到 N 条就停。**
    满足用户给的数量 / 时间窗 / 关键词条件即收口,写盘、报告。**禁止**为了「再确认一下」
    新开一轮滚动 / 翻页 / 再探一个列表页 —— 实测事故里有 17 次迭代、158 秒花在
    「补齐数据」上,最终 0 条被用上。确有缺口时,在最终回答里**如实说明缺口**,而不是
    继续空转。
21.6 **终答数据必须程序化生成,严禁凭记忆重打。**
    把抓到的数据写进文件 / 表格时,用 **Bash + Read 读抓取产物**的方式生成
    (如从 `saved_to` 路径或落盘的 JSON 里读),**禁止**把结果手抄进 Python / JS 字面量。
    实测事故里 9 行数据被硬编码在生成脚本中,没读同目录的抓取产物,导致简报被改写、
    关键句被删除 —— QC 无法与浏览器执行轨迹对账,与伪造同质。
    同理,生成 Markdown 表格时:单元格内的换行要压平(否则表格撕裂)、
    按**显示宽度**而不是字符数截断(中文按字符数算宽度会翻倍)。
"##;
