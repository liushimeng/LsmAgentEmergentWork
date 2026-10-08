//! `MCP_Web_Use(action=inspect, info="extract")`:结构化列表 / 表格抽取(第 135 轮)。
//!
//! ## 为什么要有这个维度
//!
//! 实测事故(`llaew_20261008_173357.log`,36 氪「最新文章」36 小时窗口 + AI 关键词
//! 过滤):在此之前,Agent 想把一个资讯站点的文章列表变成结构化数据,**唯一手段是
//! 手写 `eval_js`**。实测第一轮 24 次迭代里写了 17 次 eval_js,踩了四个坑:
//!
//! 1. **猜字段层级**:Main-Work 在计划里断言
//!    `itemList` 顶层有 `widgetTitle / summary / navName`,实测顶层只有
//!    `itemId,itemType,templateMaterial,route,siteId,publishTime` —— 标题字段
//!    实际嵌在 `templateMaterial` 里。照抄后全部标题为空,并导致关键词召回率
//!    14 → 9 的倒退;
//! 2. **大对象直灌**:4 次 8.5KB~19.2KB 的数组返回,一次 17440 B 落盘后从未 Read;
//! 3. **滚动 / 懒加载试错**:连续 17 次迭代试各种滚动与「加载更多」按钮,
//!    最终 9 条结果里 0 条来自那个页面;
//! 4. **收口纪律缺失**:为拿一个计划里本来就有的 URL 花掉一次完整 LLM 往返。
//!
//! 本维度把「选择器定位 + 字段投影 + 关键词过滤 + 时间窗 + 排序 + 截断」
//! 全部下沉到工具层一次完成,**在页面内过滤、只回精简字段**,直接对应第 2 条
//! 教训;`probe=true` 则直接告诉模型页面上有哪些可用的列表项选择器,
//! 对应第 1 条教训。
//!
//! ## 为什么不合并进 `extract_links`
//!
//! `extract_links`(`inspect.rs`)是**无结构**的:只给 `{href, text, context}`,
//! `context` 是一段自由文本,没有发布时间、没有摘要、没有分类标签。本维度是
//! **字段受控**的投影,两者定位不同,保留各自入口。

use serde_json::{json, Value};

use super::{eval_js_string, js_str};

/// 单个字段的抽取规则(全部可选;未命中的字段填空串而非 null)。
#[derive(Debug, Clone)]
pub(super) struct FieldSpec {
    /// 输出键名
    pub name: String,
    /// 相对 item 根节点的 CSS 选择器;`None` 表示直接取根节点自身
    pub selector: Option<String>,
    /// 取属性(如 `href` / `datetime` / `data-id`);`None` 取 innerText
    pub attr: Option<String>,
    /// 命中不到就丢弃整个 item
    pub required: bool,
    /// 字段级裁剪字符数
    pub max_chars: usize,
}

/// 过滤 / 排序参数。
#[derive(Debug, Clone, Default)]
pub(super) struct FilterSpec {
    /// 关键词(大小写不敏感子串)
    pub keywords: Vec<String>,
    /// true = 全部命中才保留;false(默认)= 任一命中
    pub match_all: bool,
    /// 在哪些字段上做关键词匹配;`None` = 全部字符串字段
    pub fields: Option<Vec<String>>,
    /// 参与时间窗与排序的字段名
    pub time_field: Option<String>,
    /// 闭区间下界
    pub since: Option<String>,
    /// 闭区间上界
    pub until: Option<String>,
    /// `field:asc` / `field:desc`
    pub sort: Option<String>,
    /// 过滤后再截一刀
    pub limit_after_filter: Option<usize>,
}

/// 字段默认裁剪长度(标题/摘要都可能很长,不给默认会顶满上下文)。
const DEFAULT_FIELD_MAX_CHARS: usize = 200;
/// `item_selector` 默认扫描上限(超过即停,防超大页面卡死)。
const DEFAULT_SCAN_CAP: usize = 1000;
/// 内部传回 Rust 的中间结果上限(关键词过滤后仍超量时二次收敛)。
const DEFAULT_RETURN_CAP: usize = 300;

/// `inspect(info="extract")` 入口。
pub(super) async fn run(
    page: &chromiumoxide::Page,
    params: &Value,
) -> std::result::Result<Value, String> {
    // probe 模式:只做选择器侦查,不抽取数据
    if params.get("probe").and_then(Value::as_bool).unwrap_or(false) {
        return run_probe(page).await;
    }

    let Some(item_selector) = params.get("item_selector").and_then(Value::as_str) else {
        return Err(
            "extract 必填:params.item_selector(列表项根节点的 CSS 选择器,例如 \".kr-flow-article-item\" \
             或 \"li.news-item, article.post\");不知道填什么就先调 probe=true 侦查页面上的候选选择器"
                .into(),
        );
    };
    let fields = parse_fields(params.get("fields"));
    let filter = parse_filter(params.get("filter"));
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, 1000) as usize)
        .unwrap_or(200);
    let scan_cap = params
        .get("scan_cap")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, 5000) as usize)
        .unwrap_or(DEFAULT_SCAN_CAP);
    // JS 侧先按 limit 收敛,避免把 1000 条原始数据传回 Rust 再截
    let return_cap = limit.min(DEFAULT_RETURN_CAP).max(1);
    let url_from = params
        .get("url_from")
        .and_then(Value::as_str)
        .unwrap_or("a")
        .to_string();

    let js = build_js(
        item_selector,
        &url_from,
        &fields,
        &filter,
        scan_cap,
        return_cap,
    );
    let raw = eval_js_string(page, &js).await?;
    finish(raw, &fields, &filter, limit)
}

// =================== 纯函数(可单测) ===================

/// 解析 `fields` 参数;缺失时给一个**不带字段假设**的通用投影。
///
/// 刻意不给「默认 title/summary 字段」—— 那等于又替模型猜了一轮页面结构,
/// 正是本次事故的根因。通用投影只回「整块文字 + 首个链接」,让模型据此写
/// `fields` 再调一次,比猜错字段名浪费一整轮便宜。
pub(super) fn parse_fields(v: Option<&Value>) -> Vec<FieldSpec> {
    let Some(obj) = v.and_then(Value::as_object) else {
        return vec![FieldSpec {
            name: "text".into(),
            selector: None,
            attr: None,
            required: false,
            max_chars: 300,
        }];
    };
    let mut out = Vec::new();
    for (name, spec) in obj {
        let selector = spec.get("selector").and_then(Value::as_str).map(str::to_string);
        let attr = spec.get("attr").and_then(Value::as_str).map(str::to_string);
        let required = spec.get("required").and_then(Value::as_bool).unwrap_or(false);
        let max_chars = spec
            .get("max_chars")
            .and_then(Value::as_u64)
            .map(|v| v.clamp(1, 4000) as usize)
            .unwrap_or(DEFAULT_FIELD_MAX_CHARS);
        out.push(FieldSpec {
            name: name.clone(),
            selector,
            attr,
            required,
            max_chars,
        });
    }
    out
}

/// 解析 `filter` 参数。
pub(super) fn parse_filter(v: Option<&Value>) -> FilterSpec {
    let Some(obj) = v.and_then(Value::as_object) else {
        return FilterSpec::default();
    };
    let strs = |k: &str| -> Vec<String> {
        obj.get(k)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    FilterSpec {
        keywords: strs("keywords"),
        match_all: obj.get("match_all").and_then(Value::as_bool).unwrap_or(false),
        fields: obj.get("fields").and_then(Value::as_array).map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        }),
        time_field: obj
            .get("time_field")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        since: obj
            .get("since")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        until: obj
            .get("until")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        sort: obj
            .get("sort")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        limit_after_filter: obj
            .get("limit_after_filter")
            .and_then(Value::as_u64)
            .map(|v| v.clamp(1, 5000) as usize),
    }
}

/// 生成页面内抽取脚本(纯函数,单测直接断言生成的脚本文本)。
///
/// 关键设计:**过滤在页面内完成,只回精简字段** —— 这正是本次事故第 2 条教训
/// (`eval_js` 返回 8.5KB~19.2KB 大对象)的对治。
pub(super) fn build_js(
    item_selector: &str,
    url_from: &str,
    fields: &[FieldSpec],
    filter: &FilterSpec,
    scan_cap: usize,
    return_cap: usize,
) -> String {
    let fields_js = Value::Array(
        fields
            .iter()
            .map(|f| {
                json!({
                    "name": f.name,
                    "selector": f.selector,
                    "attr": f.attr,
                    "required": f.required,
                    "max_chars": f.max_chars,
                })
            })
            .collect(),
    )
    .to_string();
    let kw = filter
        .keywords
        .iter()
        .map(|k| k.to_lowercase())
        .collect::<Vec<_>>();
    let kw_fields = filter.fields.clone().unwrap_or_default();
    let since_bound = filter.since.as_deref().and_then(parse_bound);
    let until_bound = filter.until.as_deref().and_then(parse_bound);

    format!(
        r#"(() => {{
  try {{
    const ITEM_SEL = {item_sel}, URL_SEL = {url_sel}, SCAN_CAP = {scan_cap}, CAP = {return_cap};
    const FIELDS = {fields_js};
    const KW = {kw}, MATCH_ALL = {match_all}, KW_FIELDS = {kw_fields};
    const TIME_FIELD = {time_field}, SINCE_RAW = {since}, UNTIL_RAW = {until}, SORT = {sort};

    // 时间边界按**页面本地时区**换算(页面上显示的发布时间就是本地墙上时钟)
    const toMs = (b) => {{
      if (b === null) return null;
      if (typeof b === 'number') return b;
      if (!Array.isArray(b) || b.length < 3) return null;
      const d = new Date(b[0], b[1] - 1, b[2], b[3] || 0, b[4] || 0, b[5] || 0);
      return isNaN(d.getTime()) ? null : d.getTime();
    }};
    const SINCE = toMs(SINCE_RAW), UNTIL = toMs(UNTIL_RAW);

    const NOW = Date.now();
    // 时间解析:相对时间(中文/英文)+ 绝对时间(ISO / "YYYY-MM-DD HH:mm" / 斜杠写法 / Unix 秒)
    const parseT = (s) => {{
      if (s === null || s === undefined) return null;
      s = String(s).trim();
      if (!s) return null;
      let m;
      if (/^(刚刚|just now)$/i.test(s)) return NOW;
      m = s.match(/^(\d+)\s*分钟前$/); if (m) return NOW - m[1] * 60000;
      m = s.match(/^(\d+)\s*分\s*钟\s*前$/); if (m) return NOW - m[1] * 60000;
      m = s.match(/^(\d+)\s*小时前$/); if (m) return NOW - m[1] * 3600000;
      m = s.match(/^(\d+)\s*天前$/); if (m) return NOW - m[1] * 86400000;
      m = s.match(/^(\d+)\s*(m|min|mins|minute|minutes)\s+ago$/i); if (m) return NOW - m[1] * 60000;
      m = s.match(/^(\d+)\s*(h|hr|hrs|hour|hours)\s+ago$/i); if (m) return NOW - m[1] * 3600000;
      m = s.match(/^(\d+)\s*(d|day|days)\s+ago$/i); if (m) return NOW - m[1] * 86400000;
      if (/^\d{{10}}$/.test(s)) return parseInt(s, 10) * 1000;      // Unix 秒
      if (/^\d{{13}}$/.test(s)) return parseInt(s, 10);            // Unix 毫秒
      let t = Date.parse(s);
      if (!isNaN(t)) return t;
      const t2 = Date.parse(s.replace(/\//g, '-').replace('T', ' '));
      return isNaN(t2) ? null : t2;
    }};
    const pad = (n) => String(n).padStart(2, '0');
    const fmtLocal = (ms) => {{
      const d = new Date(ms);
      return d.getFullYear() + '-' + pad(d.getMonth() + 1) + '-' + pad(d.getDate())
           + ' ' + pad(d.getHours()) + ':' + pad(d.getMinutes());
    }};
    const flat = (s) => String(s === null || s === undefined ? '' : s).replace(/\s+/g, ' ').trim();

    const roots = document.querySelectorAll(ITEM_SEL);
    let scanned = 0, dropped_required = 0, truncated = false;
    let rawItems = [];
    for (const root of roots) {{
      if (rawItems.length >= SCAN_CAP) {{ truncated = true; break; }}
      scanned++;
      const it = {{}};
      let bad = false;
      for (const f of FIELDS) {{
        const el = f.selector ? root.querySelector(f.selector) : root;
        let v = '';
        if (el) v = f.attr ? (el.getAttribute(f.attr) || '') : (el.innerText || el.textContent || '');
        v = flat(v).slice(0, f.max_chars);
        if (f.required && !v) {{ bad = true; break; }}
        it[f.name || ''] = v;
      }}
      if (bad) {{ dropped_required++; continue; }}
      const a = URL_SEL ? root.querySelector(URL_SEL) : null;
      it.__url = a ? (a.href || '') : '';
      rawItems.push(it);
    }}

    const matched_before_filter = rawItems.length;
    let items = rawItems;

    // 关键词过滤
    if (KW.length) {{
      const names = KW_FIELDS.length ? KW_FIELDS : FIELDS.map(f => f.name);
      items = items.filter((it) => {{
        const hay = names.map(n => flat(it[n]).toLowerCase()).join('\n');
        return MATCH_ALL ? KW.every(k => hay.indexOf(k) >= 0) : KW.some(k => hay.indexOf(k) >= 0);
      }});
    }}

    // 时间窗过滤 + 归一化(time_range 只统计**命中窗口**的条目,与 hint 文案一致)
    let unparsed_time_fields = 0, minMs = null, maxMs = null;
    if (TIME_FIELD) {{
      items = items.filter((it) => {{
        const t = parseT(it[TIME_FIELD]);
        if (t === null) {{ unparsed_time_fields++; return !(SINCE !== null || UNTIL !== null); }}
        if (SINCE !== null && t < SINCE) return false;
        if (UNTIL !== null && t > UNTIL) return false;
        if (minMs === null || t < minMs) minMs = t;
        if (maxMs === null || t > maxMs) maxMs = t;
        it[TIME_FIELD] = fmtLocal(t);
        return true;
      }});
    }}

    // 排序
    if (SORT) {{
      const idx = SORT.lastIndexOf(':');
      const key = idx > 0 ? SORT.slice(0, idx) : SORT;
      const desc = idx > 0 && SORT.slice(idx + 1).toLowerCase() === 'desc';
      items.sort((a, b) => {{
        let av = a[key], bv = b[key];
        if (TIME_FIELD && (key === TIME_FIELD)) {{
          av = parseT(a[key]); bv = parseT(b[key]);
          if (av === null) av = 0; if (bv === null) bv = 0;
          return desc ? bv - av : av - bv;
        }}
        av = flat(av).toLowerCase(); bv = flat(bv).toLowerCase();
        return desc ? (bv < av ? -1 : bv > av ? 1 : 0) : (av < bv ? -1 : av > bv ? 1 : 0);
      }});
    }}

    const total = items.length;
    const returned = items.slice(0, CAP);
    return {{
      items: returned,
      total: total,
      returned: returned.length,
      scanned: scanned,
      truncated: truncated,
      dropped_required: dropped_required,
      matched_before_filter: matched_before_filter,
      unparsed_time_fields: unparsed_time_fields,
      time_range: minMs === null ? null : {{ min: fmtLocal(minMs), max: fmtLocal(maxMs) }},
      field_names: FIELDS.map(f => f.name).concat(['__url']),
      hostname: window.location.hostname
    }};
  }} catch (e) {{
    return {{ __error: String(e && e.message ? e.message : e) }};
  }}
}})()"#,
        item_sel = js_str(item_selector),
        url_sel = js_str(url_from),
        scan_cap = scan_cap,
        return_cap = return_cap,
        fields_js = fields_js,
        kw = serde_json::to_string(&kw).unwrap_or_else(|_| "[]".into()),
        match_all = filter.match_all,
        kw_fields = serde_json::to_string(&kw_fields).unwrap_or_else(|_| "[]".into()),
        time_field = js_str(filter.time_field.as_deref().unwrap_or("")),
        since = bound_js_literal(since_bound),
        until = bound_js_literal(until_bound),
        sort = js_str(filter.sort.as_deref().unwrap_or("")),
    )
}

/// 把 `since` / `until` 参数解析成**时间戳**(**纯函数,可单测**)。
///
/// 支持三类:
/// - Unix 秒(10 位)/ Unix 毫秒(13 位)→ 直接换算成绝对毫秒,原样传给页面;
/// - `YYYY-MM-DD[ HH:mm[:ss]]`(可选 `T` 分隔 / 斜杠写法)→ 返回
///   [`Bound::Parts`],由页面按**浏览器本地时区**换算。
///
/// **为什么日期类不在 Rust 侧换算成 epoch**:页面上显示的发布时间是站点本地
/// 时区的墙上时钟,用户说的「36 小时前」自然也该按同一时区解释。若在 Rust 侧
/// 按 UTC 换算,东八区会整整差 8 小时,把本该命中的条目全部筛掉(第 135 轮
/// 开发期用 node DOM 桩实测到过:窗口 05:33 被当成 13:33,命中归零)。
///
/// 解析不出来返回 `None` —— JS 侧拿到 `null` 即**跳过该侧时间约束**(fail-open),
/// 并由 `unparsed_time_fields` 告诉模型「时间窗可能没生效」,避免静默漏数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Bound {
    /// 绝对毫秒时间戳(Unix 秒/毫秒输入)
    Ms(i64),
    /// 日历分量,由页面按本地时区换算
    Parts {
        y: i64,
        mo: i64,
        d: i64,
        hh: i64,
        mm: i64,
        ss: i64,
    },
}

pub(super) fn parse_bound(s: &str) -> Option<Bound> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    if let Ok(n) = t.parse::<i64>() {
        return Some(Bound::Ms(if t.len() <= 10 { n * 1000 } else { n }));
    }
    let body = t.replace('T', " ").replace('/', "-");
    let (date, time) = match body.split_once(' ') {
        Some((d, r)) => (d.trim(), r.trim()),
        None => (body.trim(), ""),
    };
    let mut dit = date.split('-');
    let y: i64 = dit.next()?.parse().ok()?;
    let mo: i64 = dit.next()?.parse().ok()?;
    let d: i64 = dit.next()?.parse().ok()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let (mut hh, mut mm, mut ss) = (0i64, 0i64, 0i64);
    if !time.is_empty() {
        let mut tit = time.split(':');
        hh = tit.next()?.parse().ok()?;
        mm = tit.next().unwrap_or("0").parse().ok()?;
        ss = tit.next().unwrap_or("0").parse().ok()?;
    }
    if !(0..=23).contains(&hh) || !(0..=59).contains(&mm) || !(0..=59).contains(&ss) {
        return None;
    }
    Some(Bound::Parts {
        y,
        mo,
        d,
        hh,
        mm,
        ss,
    })
}

/// Howard Hinnant 的 civil-days 算法(不引 chrono,纯整数运算)。
///
/// 仅供单测交叉验证 `Bound::Parts` 与绝对时间戳的换算关系;运行时换算在页面内
/// 按本地时区完成(见 [`parse_bound`] 的文档)。
#[cfg(test)]
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 把 [`Bound`] 序列化成可直接嵌入 JS 的字面量(绝对值或日历分量数组)。
fn bound_js_literal(b: Option<Bound>) -> String {
    match b {
        Some(Bound::Ms(ms)) => ms.to_string(),
        Some(Bound::Parts { y, mo, d, hh, mm, ss }) => {
            format!("[{y},{mo},{d},{hh},{mm},{ss}]")
        }
        None => "null".to_string(),
    }
}

/// 收尾加工:二次截断 + 组装诊断信息(纯函数,可单测)。
fn finish(
    raw: Value,
    fields: &[FieldSpec],
    filter: &FilterSpec,
    limit: usize,
) -> std::result::Result<Value, String> {
    if let Some(err) = raw.get("__error").and_then(Value::as_str) {
        return Err(format!("extract 页面内脚本异常: {err}"));
    }
    let mut out = raw.as_object().cloned().unwrap_or_default();
    let items = out
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // `limit_after_filter` 在 JS 侧未生效时兜底(当前 JS 已按 CAP 截,这里做最终收敛)
    let mut final_items = items;
    if let Some(n) = filter.limit_after_filter {
        final_items.truncate(n);
    }
    final_items.truncate(limit);
    out.insert("items".into(), Value::Array(final_items));
    out.insert("limit".into(), json!(limit));

    // 条件化提示:让模型知道下一步该干什么,而不是「拿到 N 条就不动了」
    let snapshot = Value::Object(out.clone());
    let hints = build_hints(&snapshot, fields, filter);
    if !hints.is_empty() {
        out.insert("hint".into(), json!(hints.join(" ")));
    }
    Ok(Value::Object(out))
}

/// 组装条件化 hint。
fn build_hints(out: &Value, fields: &[FieldSpec], filter: &FilterSpec) -> Vec<String> {
    let mut hints = Vec::new();
    let scanned = out.get("scanned").and_then(Value::as_u64).unwrap_or(0);
    let total = out.get("total").and_then(Value::as_u64).unwrap_or(0);
    let returned = out.get("returned").and_then(Value::as_u64).unwrap_or(0);
    let truncated = out
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if total == 0 && scanned == 0 {
        hints.push(
            "item_selector 没匹配到任何元素;先调 probe=true 侦查页面上的候选选择器,或用 inspect(elements) 看结构"
                .to_string(),
        );
    }
    if truncated {
        hints.push(
            "已达 scan_cap 上限、页面可能还有更多条目;需要全量时调大 scan_cap,或先滚动/分页再抽一次"
                .to_string(),
        );
    }
    if returned < total {
        hints.push(format!(
            "共 {total} 条命中,只内联了前 {returned} 条;要全量请分批抽取或调大 limit"
        ));
    }
    if out
        .get("unparsed_time_fields")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        > 0
    {
        hints.push(
            "有条目的时间字段无法解析,时间窗可能未生效;用 inspect(dom) 看该字段真实格式后,在 fields 里改用 attr(如 time[datetime])取值"
                .to_string(),
        );
    }
    if filter.since.is_some() || filter.until.is_some() {
        if let Some(tr) = out.get("time_range") {
            if !tr.is_null() {
                hints.push(format!(
                    "时间窗已生效,命中范围 {} ~ {}",
                    tr.get("min").and_then(Value::as_str).unwrap_or("?"),
                    tr.get("max").and_then(Value::as_str).unwrap_or("?")
                ));
            }
        }
    }
    if fields.len() == 1 && fields[0].selector.is_none() {
        hints.push(
            "当前是通用投影(只有 text + __url);按本页实际结构补 params.fields(如 {title:{selector:\"h3 a\",required:true},time:{selector:\"time\",attr:\"datetime\"}})再抽一次更准"
                .to_string(),
        );
    }
    hints
}

/// 选择器侦查模式:列出页面上重复出现的类名与候选列表项选择器。
///
/// 对应本次事故的第 1 条教训(猜字段层级 / 猜选择器)。让模型一次调用就知道
/// 该用哪个 `item_selector`,省掉实测里 5~8 轮盲试。
async fn run_probe(page: &chromiumoxide::Page) -> std::result::Result<Value, String> {
    let js = r#"(() => {
  try {
    const tally = {};
    const nodes = document.querySelectorAll('*');
    const N = Math.min(nodes.length, 1200);
    for (let i = 0; i < N; i++) {
      const cls = nodes[i].className;
      if (typeof cls !== 'string' || !cls) continue;
      for (const c of cls.split(/\s+/)) {
        if (!c || c.length > 40) continue;
        tally[c] = (tally[c] || 0) + 1;
      }
    }
    const repeated = Object.keys(tally)
      .map(k => ({ cls: k, count: tally[k] }))
      .filter(x => x.count >= 4)
      .sort((a, b) => b.count - a.count)
      .slice(0, 40);
    const interesting = repeated.filter(x =>
      /item|list|card|news|article|post|flow|feed|row|item-|-item|main/i.test(x.cls));
    const probeSel = (sel) => { try { return document.querySelectorAll(sel).length; } catch(e){ return -1; } };
    const common = [
      '.kr-flow-article-item','article','li','[class*=item]','[class*=card]',
      '[class*=news]','[class*=article]','[class*=list]','[class*=post]','[class*=feed]'
    ].map(sel => ({ selector: sel, count: probeSel(sel) }));
    return {
      total_elements: nodes.length,
      sampled: N,
      repeated_classes: repeated,
      likely_item_classes: interesting,
      common_selectors: common,
      hostname: window.location.hostname,
      hint: '从 likely_item_classes 里挑 count 最大且名字像列表项的类名作为 item_selector;仍不确定时用 inspect(elements, selector) 验证'
    };
  } catch (e) { return { __error: String(e && e.message ? e.message : e) }; }
})()"#;
    let raw = eval_js_string(page, js).await?;
    if let Some(err) = raw.get("__error").and_then(Value::as_str) {
        return Err(format!("extract(probe) 页面内脚本异常: {err}"));
    }
    Ok(raw)
}
