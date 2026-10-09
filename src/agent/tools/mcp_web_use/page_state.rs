//! `MCP_Web_Use(action=inspect, info="page_state")`:页面内 JS 全局状态直读(第 135 轮)。
//!
//! ## 为什么要有这个维度
//!
//! 现代资讯站点的列表数据几乎都是 **SSR 注水**的:页面加载时把整份 JSON 塞进
//! `window.__NEXT_DATA__` / `window.initialState` / `window.__NUXT__` 等全局变量,
//! 之后由前端 JS 渲染成 DOM。实测(`llaew_20261008_173357.log` 36 氪任务)里
//! 第二轮成功路径正是 `eval_js` 直读 `window.initialState.homeData.data.homeFlow.data.itemList`
//! —— **一次调用、1.4 秒、零滚动、零新开页面**,把整件事做完。
//!
//! 但在此之前,模型只能**猜全局变量名**:全仓 `grep __NEXT_DATA__` 零命中,
//! 工具 description 里也只字未提。猜错就退化成 DOM 遍历,而虚拟滚动 / 懒加载
//! 列表页的 DOM 是残缺的 —— 实测第一轮为此烧掉 17 次迭代、158 秒,最终 9 条结果
//! 里 0 条来自那个页面。
//!
//! 本维度把「有哪些注水点 → 各是什么结构 → 取哪条路径」变成一次可枚举的调用,
//! 并用**三闸裁剪**保证不重演 `eval_js` 返回 8.5KB~19.2KB 大对象的旧问题。
//!
//! ## 关键取舍:返回 `dropped_paths`
//!
//! 被裁掉的部分**必须显式告诉模型**,否则它会误以为「数据就这么多」,
//! 从而在错误的假设上继续下游推理。`dropped_paths` 让模型据此自己写精确路径
//! 的 `eval_js` 取子集,而不是重新猜一遍字段层级。

use serde_json::Value;

use super::eval_js_string;

/// 内建注水点候选(顺序即优先级,取第一个命中的)。
const DEFAULT_CANDIDATES: &[&str] = &[
    "__NEXT_DATA__",
    "__NUXT__",
    "__INITIAL_STATE__",
    "__APOLLO_STATE__",
    "__PRELOADED_STATE__",
    "__remixContext",
    "initialState",
    "__INITIAL_DATA__",
];

/// 默认体积闸门(字节)。
const DEFAULT_MAX_BYTES: usize = 20_000;
/// 默认深度闸门。
const DEFAULT_MAX_DEPTH: usize = 8;
/// 默认数组元素闸门。
const DEFAULT_MAX_ARRAY_ITEMS: usize = 200;

/// `inspect(info="page_state")` 入口。
pub(super) async fn run(
    page: &chromiumoxide::Page,
    params: &Value,
) -> std::result::Result<Value, String> {
    let keys: Vec<String> = params
        .get("keys")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .filter(|v: &Vec<String>| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_CANDIDATES.iter().map(|s| s.to_string()).collect());
    let probe_window = params
        .get("probe_window")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let max_bytes = params
        .get("max_bytes")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1024, 500_000) as usize)
        .unwrap_or(DEFAULT_MAX_BYTES);
    let max_depth = params
        .get("max_depth")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, 20) as usize)
        .unwrap_or(DEFAULT_MAX_DEPTH);
    let max_items = params
        .get("max_array_items")
        .and_then(Value::as_u64)
        .map(|v| v.clamp(1, 5000) as usize)
        .unwrap_or(DEFAULT_MAX_ARRAY_ITEMS);

    let js = build_js(&keys, probe_window, max_bytes, max_depth, max_items);
    let raw = eval_js_string(page, &js).await?;
    if let Some(err) = raw.get("__error").and_then(Value::as_str) {
        return Err(format!("page_state 页面内脚本异常: {err}"));
    }
    Ok(raw)
}

/// 生成页面内脚本(纯函数,单测直接断言脚本文本)。
pub(super) fn build_js(
    keys: &[String],
    probe_window: bool,
    max_bytes: usize,
    max_depth: usize,
    max_items: usize,
) -> String {
    format!(
        r#"(() => {{
  try {{
    const KEYS = {keys};
    const PROBE = {probe};
    const MAX_BYTES = {max_bytes}, MAX_DEPTH = {max_depth}, MAX_ITEMS = {max_items};
    const dropped = [];
    let outBytes = {{ n: 0 }};

    // JSON-LD(结构化数据标准位):顺带提取,常常直接就是文章列表或单篇元信息
    const jsonld = [];
    for (const s of document.querySelectorAll('script[type="application/ld+json"]')) {{
      if (jsonld.length >= 5) break;
      try {{
        const j = JSON.parse(s.textContent || 'null');
        if (j) jsonld.push(shrink(j, 'jsonld[' + jsonld.length + ']', 0));
      }} catch (e) {{}}
    }}

    // 收缩器:三闸(体积 / 深度 / 数组长度),被裁处显式记进 dropped
    function shrink(v, path, depth) {{
      outBytes.n += 8;
      if (outBytes.n > MAX_BYTES) {{ dropped.push(path + ' …(体积上限)'); return '…(体积上限)'; }}
      if (depth > MAX_DEPTH) {{ dropped.push(path + ' …(深度上限)'); return '…(深度上限)'; }}
      if (Array.isArray(v)) {{
        const out = [];
        for (let i = 0; i < v.length; i++) {{
          if (i >= MAX_ITEMS) {{ dropped.push(path + '[' + i + '..' + v.length + ']'); break; }}
          out.push(shrink(v[i], path + '[' + i + ']', depth + 1));
        }}
        return out;
      }}
      if (v && typeof v === 'object') {{
        const o = {{}};
        for (const k of Object.keys(v)) o[k] = shrink(v[k], path + '.' + k, depth + 1);
        return o;
      }}
      if (typeof v === 'string' && v.length > 500) return v.slice(0, 500) + '…(' + v.length + ')';
      return v;
    }}

    const globals = {{}};
    const found = [];
    for (const k of KEYS) {{
      let v = null;
      try {{ v = window[k]; }} catch (e) {{ continue; }}
      if (v === null || v === undefined) continue;
      found.push(k);
      globals[k] = shrink(v, k, 0);
      if (Object.keys(globals).length >= 3) break;
    }}

    // 探测:扫出 window 上所有 __ 前缀键名 + 一层结构,让模型不必先猜名字
    let window_globals = [];
    if (PROBE) {{
      const names = Object.getOwnPropertyNames(window).filter(n => /^__|^initialState$|^_app/i.test(n) && !/^__laew/.test(n));
      for (const n of names.slice(0, 40)) {{
        let shape = null;
        try {{
          const v = window[n];
          shape = (v && typeof v === 'object')
            ? Object.keys(v).slice(0, 30)
            : (typeof v);
        }} catch (e) {{ shape = 'unreadable'; }}
        window_globals.push({{ name: n, shape: shape }});
      }}
    }}

    return {{
      found: found,
      globals: globals,
      window_globals: window_globals,
      jsonld: jsonld,
      dropped_paths: dropped.slice(0, 40),
      truncated: dropped.length > 0,
      returned_bytes: outBytes.n,
      hostname: window.location.hostname,
      hint: dropped.length
        ? '有内容因体积/深度/数组长度上限被裁,见 dropped_paths;要取精确子集请用 eval_js 自带路径表达式(如 JSON.stringify(window.__NEXT_DATA__.props.pageProps.list.slice(0,20))),不要直接把整棵对象塞进工具参数'
        : '按 globals 里的结构选定路径后,用 eval_js 取精确子集;不要把整棵对象原样搬进后续工具参数'
    }};
  }} catch (e) {{
    return {{ __error: String(e && e.message ? e.message : e) }};
  }}
}})()"#,
        keys = serde_json::to_string(keys).unwrap_or_else(|_| "[]".into()),
        probe = probe_window,
        max_bytes = max_bytes,
        max_depth = max_depth,
        max_items = max_items,
    )
}

/// 单测用:确认内建候选非空且互不重复。
#[cfg(test)]
pub(crate) fn builtin_candidates() -> &'static [&'static str] {
    DEFAULT_CANDIDATES
}
