//! 窗口操控工具(WindowUse Agent 专用):WindowList / WindowFind / WindowInspect /
//! WindowAction / WindowScreenshot。
//!
//! - 平台差异封闭在 `agent::window` 驱动层,本模块只做参数校验、
//!   `spawn_blocking` 包裹(UIA COM / AX IPC 可能阻塞)与输出截断;
//! - 控件树输出双重截断(节点数 ≤ 500,序列化 ≤ 32KB),防止 tool_result
//!   打爆上下文(对齐 overflow.rs 排水策略);
//! - 错误信息 LLM 可读:权限缺失给开权限步骤,控件未找到给重新检视提示;
//! - **WindowFind**(2026-09-16 第 56 轮):按「标题/进程名子串」+ 模糊匹配打分排序,
//!   返回最佳匹配窗口的 id(LLM 拿到 WindowList 后常不知道取哪个,这一步省一
//!   次「读 JSON 全列表再选」);
//! - **WindowScreenshot**(2026-09-16 第 56 轮):跨平台截图,落盘 PNG 后返回路径,
//!   为后续 OCR / 视觉验证铺路。macOS 走 screencapture(白名单内),Windows 走
//!   PowerShell + System.Drawing,Linux 走 import/scrot(尽力而为)。
//!
//! 设计见 `docs/WindowUse桌面窗口操控Agent/01-设计与解决方案.md` §2.4。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::Tool;
use crate::agent::window::{current_driver, ControlAction, ControlNode, WindowInfo};
use crate::error::{AgentError, Result};

/// 控件树节点数上限(超出截断)。
const MAX_TREE_NODES: usize = 500;
/// 控件树序列化字节上限(超出截断)。
const MAX_TREE_BYTES: usize = 32 * 1024;
/// 窗口列表条数上限。
const MAX_WINDOWS: usize = 200;

fn tool_err(tool: &str, reason: impl Into<String>) -> AgentError {
    AgentError::ToolExecution {
        tool: tool.into(),
        reason: reason.into(),
    }
}

fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn require_str<'a>(args: &'a Value, key: &str, tool: &str) -> Result<&'a str> {
    get_str(args, key).ok_or_else(|| tool_err(tool, format!("缺少 string 类型参数 {key}")))
}

/// 统计控件树节点数。
fn count_nodes(node: &ControlNode) -> usize {
    1 + node.children.iter().map(count_nodes).sum::<usize>()
}

/// 深度优先裁剪子树,使总节点数 ≤ budget。
fn prune_nodes(node: &mut ControlNode, budget: &mut usize) {
    let mut kept = Vec::with_capacity(node.children.len());
    for mut child in std::mem::take(&mut node.children) {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        prune_nodes(&mut child, budget);
        kept.push(child);
    }
    node.children = kept;
}

/// 把控件树序列化为带截断标注的 JSON 字符串。
fn tree_to_json(mut root: ControlNode) -> String {
    let total = count_nodes(&root);
    let mut truncated = false;
    if total > MAX_TREE_NODES {
        let mut budget = MAX_TREE_NODES;
        prune_nodes(&mut root, &mut budget);
        truncated = true;
    }
    let mut s = serde_json::to_string_pretty(&root).unwrap_or_else(|_| "{}".into());
    if s.len() > MAX_TREE_BYTES {
        s.truncate(MAX_TREE_BYTES);
        s.push_str("\n... [truncated: 输出超过 32KB]");
        truncated = true;
    }
    if truncated {
        s.push_str(&format!(
            "\n[truncated] 控件树共 {total} 个节点,仅展示前 {} 个;可用 max_depth 调小或 filter 过滤后重试",
            MAX_TREE_NODES.min(total)
        ));
    }
    s
}

/// 驱动不可用/缺权限时的前置检查:有 permission_hint 时优先返回引导文案。
fn driver_preflight(tool: &str) -> Result<()> {
    let driver = current_driver();
    if let Some(hint) = driver.permission_hint() {
        return Err(tool_err(tool, hint));
    }
    Ok(())
}

async fn run_blocking<F>(tool: &str, f: F) -> Result<String>
where
    F: FnOnce() -> Result<String> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| tool_err(tool, format!("窗口驱动任务 join 失败: {e}")))?
}

// ===================== WindowList =====================

/// 枚举可见顶层窗口。
pub struct WindowListTool;

#[async_trait]
impl Tool for WindowListTool {
    fn name(&self) -> &str {
        "WindowList"
    }

    fn description(&self) -> &str {
        "枚举当前桌面全部可见顶层窗口,返回 JSON 数组(id/title/进程名/PID/位置尺寸)。\n\
         - filter 可选:按窗口标题或进程名子串过滤(大小写不敏感)。\n\
         - 返回的 id 是不透明标识,仅供 WindowInspect/WindowAction 回传使用。\n\
         - macOS 需要「辅助功能」权限;未授权时返回可读的开权限引导。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filter": { "type": "string", "description": "可选,窗口标题/进程名子串过滤" }
            },
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let filter = get_str(&args, "filter").map(str::to_string);
        // 2026-09-15 第 53 轮 P2 修复:WindowList 不调用 driver_preflight。
        // 原设计是「缺权限/缺依赖时前置报错」,但实际上:
        //   - Windows:WindowList 无需任何授权(UIA 仅 inspect/act 需权限);
        //   - macOS:WindowList 走 CoreGraphics(CGWindowListCopyWindowInfo),
        //     不需要 AX 无障碍权限(即使未授权也可枚举);
        //   - Fallback (Linux):缺 wmctrl 时由 list_windows 自身返回结构化错误。
        // 旧 preflight 曾误把 WindowList 也 fail-closed(源于「macOS 26 移除 AX」的误判),
        // 但 WindowList 完全可用 —— 改由 list_windows 内部自行处理平台差异。
        // WindowInspect / WindowAction 仍保留 driver_preflight(真需要权限)。
        run_blocking(self.name(), move || {
            let driver = current_driver();
            let mut wins = driver.list_windows(filter.as_deref())?;
            let total = wins.len();
            if total > MAX_WINDOWS {
                wins.truncate(MAX_WINDOWS);
            }
            let mut out = serde_json::to_string_pretty(&wins).unwrap_or_else(|_| "[]".into());
            if total > MAX_WINDOWS {
                out.push_str(&format!(
                    "\n[truncated] 共 {total} 个窗口,仅展示前 {MAX_WINDOWS} 个;请用 filter 缩小范围"
                ));
            }
            if wins.is_empty() {
                out.push_str("\n(无匹配窗口;若预期有窗口,请确认目标应用已启动且未最小化)");
            }
            Ok(out)
        })
        .await
    }
}

// ===================== WindowFind =====================
//
// 2026-09-16 第 56 轮:WindowUse 工具集新增 WindowFind。
// 背景:WindowList 返回全量窗口列表(JSON 数组),LLM 需要自己读 JSON 后再选
// 「微信是 id=xxx 那个」,这一步常消耗数百 token + 一次误选 → WindowFind 直接
// 接「按标题/进程名子串找一个窗口」,返回最佳匹配的 id(单一字符串,极小上下文)。
// - match_mode:
//   - "exact"    标题或进程名完全相等(忽略大小写);有多个时取首个
//   - "contains" 子串匹配(默认,大小写不敏感)
//   - "fuzzy"    子串 + 字符级相似度打分排序,返回 top-1
// - 返回:JSON 字符串 {"window_id":"...", "title":"...", "process_name":"...",
//   "score":0.95, "matched_field":"title"};若无匹配返回 error 引导改宽松策略。
//
// 不引入新依赖,字符串匹配走 std::str 与简单 Damerau-Levenshtein 上界剪枝
// (对窗口标题这种几十字符的串足够,fuzzy 仅用于 top-1 而非全排序)。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchMode {
    Exact,
    Contains,
    Fuzzy,
}

impl MatchMode {
    fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "exact" => Self::Exact,
            "fuzzy" => Self::Fuzzy,
            _ => Self::Contains,
        }
    }
}

struct ScoredHit<'a> {
    info: &'a WindowInfo,
    score: f64,
    matched_field: &'static str, // "title" / "process"
}

fn score_exact<'a>(query_lower: &str, info: &'a WindowInfo) -> Option<ScoredHit<'a>> {
    let title_l = info.title.to_lowercase();
    let proc_l = info.process_name.to_lowercase();
    if title_l == query_lower {
        Some(ScoredHit { info, score: 1.0, matched_field: "title" })
    } else if proc_l == query_lower {
        Some(ScoredHit { info, score: 1.0, matched_field: "process" })
    } else {
        None
    }
}

fn score_contains<'a>(query_lower: &str, info: &'a WindowInfo) -> Option<ScoredHit<'a>> {
    let title_l = info.title.to_lowercase();
    let proc_l = info.process_name.to_lowercase();
    if title_l.contains(query_lower) {
        let q_chars = query_lower.chars().count() as f64;
        let denom = title_l.chars().count().max(1) as f64;
        let s = (q_chars / denom).clamp(0.3, 0.95);
        Some(ScoredHit { info, score: s, matched_field: "title" })
    } else if proc_l.contains(query_lower) {
        let q_chars = query_lower.chars().count() as f64;
        let denom = proc_l.chars().count().max(1) as f64;
        let s = (q_chars / denom).clamp(0.3, 0.95);
        Some(ScoredHit { info, score: s, matched_field: "process" })
    } else {
        None
    }
}

/// 简化 Levenshtein 距离(无 transposition,纯 DP),
/// 仅用于 ≤ 64 字符的窗口标题;上界剪枝:长度差 > 8 直接返回 max_len。
fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let max_len = m.max(n);
    let min_len = m.min(n);
    if max_len - min_len > 8 {
        return max_len;
    }
    let mut prev2: Vec<usize> = (0..=n).collect();
    let mut prev1: Vec<usize> = vec![0; n + 1];
    for i in 1..=m {
        prev1[0] = i;
        for j in 1..=n {
            let cost = if a_chars[i - 1] == b_chars[j - 1] { 0 } else { 1 };
            let del = prev1[j - 1] + 1; // insertion
            let ins = prev2[j] + 1; // deletion
            let sub = prev2[j - 1] + cost;
            prev1[j] = del.min(ins).min(sub);
        }
        std::mem::swap(&mut prev1, &mut prev2);
    }
    prev2[n]
}

fn score_fuzzy<'a>(query: &str, info: &'a WindowInfo) -> Option<ScoredHit<'a>> {
    let q = query.to_lowercase();
    let title_l = info.title.to_lowercase();
    let proc_l = info.process_name.to_lowercase();
    let dt = damerau_levenshtein(&q, &title_l);
    let dp = damerau_levenshtein(&q, &proc_l);
    let max_len = q.chars().count().max(info.title.chars().count()).max(1);
    let score_t = (1.0 - (dt as f64) / (max_len as f64)).max(0.0);
    let score_p = (1.0 - (dp as f64) / (max_len as f64)).max(0.0);
    if score_t < 0.2 && score_p < 0.2 {
        return None;
    }
    if score_t >= score_p {
        Some(ScoredHit { info, score: score_t, matched_field: "title" })
    } else {
        Some(ScoredHit { info, score: score_p, matched_field: "process" })
    }
}

/// 按窗口信息列表 + 模式 + 查询词匹配,返回 top-1。
fn pick_top_hit<'a>(
    wins: &'a [WindowInfo],
    query: &str,
    mode: MatchMode,
) -> Option<ScoredHit<'a>> {
    let query_lower = query.to_lowercase();
    wins.iter()
        .filter_map(|w| match mode {
            MatchMode::Exact => score_exact(&query_lower, w),
            MatchMode::Contains => score_contains(&query_lower, w),
            MatchMode::Fuzzy => score_fuzzy(query, w),
        })
        .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal))
}

/// 按标题/进程名子串找一个窗口,返回最佳匹配的 id。
pub struct WindowFindTool;

#[async_trait]
impl Tool for WindowFindTool {
    fn name(&self) -> &str {
        "WindowFind"
    }

    fn description(&self) -> &str {
        "按标题/进程名子串查找一个窗口,直接返回最佳匹配的 window_id(单一字符串)。\n\
         - query 必填:窗口标题或进程名的查询词(大小写不敏感)\n\
         - match_mode 可选:exact / contains / fuzzy(默认 contains);\n\
           exact = 完全相等;contains = 子串命中;fuzzy = 字符级相似度排序。\n\
         - 返回 JSON:{\"window_id\":\"...\",\"title\":\"...\",\"process_name\":\"...\",\
           \"score\":0.95,\"matched_field\":\"title\"}\n\
         用于:WindowList 返回后,不想逐条读 JSON 自己选 — 直接传「微信」/「WeChat」即可。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "查询词,匹配标题或进程名(大小写不敏感)" },
                "match_mode": {
                    "type": "string",
                    "enum": ["exact", "contains", "fuzzy"],
                    "description": "匹配模式,默认 contains"
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let query = require_str(&args, "query", self.name())?.to_string();
        let mode_str_owned = get_str(&args, "match_mode").unwrap_or("contains").to_string();
        let mode = MatchMode::parse(&mode_str_owned);
        // WindowFind 仅基于 WindowList 结果(走 CoreGraphics,无授权要求),
        // 不调用 driver_preflight,避免误把 macOS AX 未授权也 fail-closed。
        run_blocking(self.name(), move || {
            let driver = current_driver();
            let wins = driver.list_windows(None)?;
            if wins.is_empty() {
                return Err(tool_err(
                    "WindowFind",
                    "无可见窗口,请确认桌面有目标应用在前台",
                ));
            }
            match pick_top_hit(&wins, &query, mode) {
                Some(hit) => {
                    // 2026-09-16 第 58 轮 P1-B:title 为空但 process_name 命中时,
                    // 输出 note 字段说明 WeChat/部分 Electron 应用 NSWindow title
                    // 私有化的已知行为,引导 LLM 继续 WindowInspect 而不是放弃。
                    let note = if hit.info.title.is_empty()
                        && hit.matched_field == "process"
                    {
                        Some(
                            "macOS 上 WeChat / 部分 Electron 应用 NSWindow title \
                             可能为空(CoreGraphics 拿不到),这是正常结果。\
                             窗口已通过 process_name 成功定位,可以直接调用 \
                             WindowInspect(window_id) 继续检视控件树。"
                        )
                    } else if hit.info.title.is_empty() {
                        Some(
                            "title 为空,可能 NSWindow 私有化;通过 process_name 匹配,\
                             可继续 WindowInspect"
                        )
                    } else {
                        None
                    };
                    let mut body = json!({
                        "window_id": hit.info.id,
                        "title": hit.info.title,
                        "process_name": hit.info.process_name,
                        "pid": hit.info.pid,
                        "score": hit.score,
                        "matched_field": hit.matched_field,
                        "match_mode": match mode {
                            MatchMode::Exact => "exact",
                            MatchMode::Contains => "contains",
                            MatchMode::Fuzzy => "fuzzy",
                        },
                    });
                    if let Some(n) = note {
                        body["note"] = json!(n);
                    }
                    Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
                }
                None => {
                    let titles: Vec<String> = wins
                        .iter()
                        .take(10)
                        .map(|w| format!("{} ({})", w.title, w.process_name))
                        .collect();
                    Err(tool_err(
                        "WindowFind",
                        format!(
                            "无匹配 query={query:?} mode={mode_str_owned:?} 的窗口;当前可见前 10 个:{titles:?}。\
                             建议:改用 match_mode=contains 或更短 query,或先用 WindowList 自行确认可见窗口"
                        ),
                    ))
                }
            }
        })
        .await
    }
}

// ===================== WindowScreenshot =====================
//
// 2026-09-16 第 56 轮:WindowUse 工具集新增 WindowScreenshot。
// 背景:为后续 OCR / 视觉验证铺路 —— 当 WindowInspect 拿不到可读控件树(如
// Electron 应用、canvas 渲染、自绘控件)时,截图 + 视觉模型是唯一出路。本工具
// 仅做截图落盘,不做视觉识别(避免引入 OCR / ML 依赖,本轮只提供原图)。
//
// 平台实现(本工具不依赖 WindowDriver,直接走系统命令,白名单内可放行):
// - macOS: `screencapture -x -t png <path>`(区域截图: `-R x,y,w,h`)。
// - Windows: PowerShell + System.Drawing(Graphics.CopyFromScreen)。
// - Linux: `import` (ImageMagick) 或 `scrot`(尽力而为,缺则失败提示)。
//
// 参数:
// - output_path:可选,默认 `/tmp/laew_screenshot_<时间戳>.png`;若指定
//   目录不存在则自动 mkdir。
// - region:可选 JSON {x,y,width,height},仅截指定区域;不支持则报错。
// 返回:JSON {"path":"...","size_bytes":N,"created_at":"..."}。

/// 平台默认截图命令(白名单内)。
#[cfg(target_os = "macos")]
fn default_screenshot_command(output_path: &str, region: Option<&Value>) -> String {
    if let Some(reg) = region {
        let x = reg.get("x").and_then(Value::as_i64).unwrap_or(0);
        let y = reg.get("y").and_then(Value::as_i64).unwrap_or(0);
        let w = reg.get("width").and_then(Value::as_i64).unwrap_or(0);
        let h = reg.get("height").and_then(Value::as_i64).unwrap_or(0);
        format!("screencapture -x -R{x},{y},{w},{h} -t png {output_path}")
    } else {
        format!("screencapture -x -t png {output_path}")
    }
}

#[cfg(windows)]
fn default_screenshot_command(output_path: &str, region: Option<&Value>) -> String {
    let script = if let Some(reg) = region {
        let x = reg.get("x").and_then(Value::as_i64).unwrap_or(0);
        let y = reg.get("y").and_then(Value::as_i64).unwrap_or(0);
        let w = reg.get("width").and_then(Value::as_i64).unwrap_or(0);
        let h = reg.get("height").and_then(Value::as_i64).unwrap_or(0);
        format!(
            "Add-Type -AssemblyName System.Drawing; \
             $bmp = New-Object System.Drawing.Bitmap {w},{h}; \
             $g = [System.Drawing.Graphics]::FromImage($bmp); \
             $g.CopyFromScreen({x},{y},0,0,$bmp.Size); \
             $bmp.Save('{output_path}',[System.Drawing.Imaging.ImageFormat]::Png); \
             $g.Dispose(); $bmp.Dispose()"
        )
    } else {
        format!(
            "Add-Type -AssemblyName System.Windows.Forms,System.Drawing; \
             $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds; \
             $bmp = New-Object System.Drawing.Bitmap $b.Width,$b.Height; \
             $g = [System.Drawing.Graphics]::FromImage($bmp); \
             $g.CopyFromScreen($b.Location,[Drawing.Point]::Empty,$bmp.Size); \
             $bmp.Save('{output_path}',[System.Drawing.Imaging.ImageFormat]::Png); \
             $g.Dispose(); $bmp.Dispose()"
        )
    };
    format!("powershell -NoProfile -NonInteractive -Command \"{script}\"")
}

#[cfg(not(any(target_os = "macos", windows)))]
fn default_screenshot_command(_output_path: &str, _region: Option<&Value>) -> String {
    // Linux:ImageMagick import(常用);失败可改 scrot。
    "import -window root /tmp/laew_screen.png".to_string()
}

/// 落盘截图(走系统命令,需 WindowUse Bash 白名单内)。
pub struct WindowScreenshotTool;

#[async_trait]
impl Tool for WindowScreenshotTool {
    fn name(&self) -> &str {
        "WindowScreenshot"
    }

    fn description(&self) -> &str {
        "截图落盘,返回 PNG 文件路径(为后续 OCR / 视觉验证铺路)。\n\
         - output_path 可选:默认 /tmp/laew_screenshot_<时间戳>.png;目录不存在自动 mkdir。\n\
         - region 可选:{\"x\":N,\"y\":N,\"width\":N,\"height\":N} 仅截指定区域。\n\
         平台差异(本工具自动选用):macOS screencapture / Windows PowerShell + System.Drawing /\n\
         Linux ImageMagick import(尽力而为)。仅做截图,不做视觉识别 —— 若需 OCR/视觉,后续扩展。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "output_path": { "type": "string", "description": "输出 PNG 路径,默认 /tmp/laew_screenshot_<ts>.png" },
                "region": {
                    "type": "object",
                    "properties": {
                        "x": { "type": "integer" },
                        "y": { "type": "integer" },
                        "width": { "type": "integer" },
                        "height": { "type": "integer" }
                    },
                    "required": ["x", "y", "width", "height"],
                    "description": "可选截图区域"
                }
            },
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let output_path = get_str(&args, "output_path")
            .map(str::to_string)
            .unwrap_or_else(|| format!("/tmp/laew_screenshot_{ts}.png"));
        let region = args.get("region").cloned();

        // 落盘前确保父目录存在(纯 Rust 调用,不进入白名单校验)。
        if let Some(parent) = std::path::Path::new(&output_path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    tool_err(self.name(), format!("创建父目录 {:?} 失败: {}", parent, e))
                })?;
            }
        }

        let command = default_screenshot_command(&output_path, region.as_ref());
        // 走 BashTool 执行(WindowUse 白名单模式由 WindowUseRunner 提前开启,
        // 此处直接调;在 WindowUse Runner 上下文里 LAEW_WINDOW_USE_MODE 已设 1)。
        let bash = crate::agent::tools::bash::BashTool;
        let bash_args = json!({
            "command": command,
            "timeout_ms": 30000
        });
        let output = bash.execute(bash_args).await?;

        // 校验产物文件存在
        let meta = std::fs::metadata(&output_path)
            .map_err(|e| tool_err(self.name(), format!("截图未生成: {};命令输出={}", e, output)))?;
        let size_bytes = meta.len();
        let body = json!({
            "path": output_path,
            "size_bytes": size_bytes,
            "created_at_unix": ts,
            "platform": std::env::consts::OS,
            "command": command,
        });
        Ok(serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into()))
    }
}

// ===================== WindowInspect =====================

/// 枚举指定窗口的控件树。
pub struct WindowInspectTool;

#[async_trait]
impl Tool for WindowInspectTool {
    fn name(&self) -> &str {
        "WindowInspect"
    }

    fn description(&self) -> &str {
        "枚举指定窗口的控件树(Windows 走 UI Automation,macOS 走 Accessibility),\n\
         返回 JSON 树:每个控件含 path(如 /0/2/1,供 WindowAction 定位)、role、name、value、\n\
         bounds、actions(支持的动作列表)、children。\n\
         - window_id 必填:WindowList 返回的窗口 id。\n\
         - max_depth 可选:遍历深度,默认 3,范围 1-12。控件很多时先用小深度+filter。\n\
         - filter 可选:按控件名/角色子串过滤,只保留命中控件及其祖先链。\n\
         建议先检视再操作;树过大时输出会自动截断并标注。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "window_id": { "type": "string", "description": "WindowList 返回的窗口 id" },
                "max_depth": { "type": "integer", "minimum": 1, "maximum": 12, "description": "遍历深度,默认 3" },
                "filter": { "type": "string", "description": "可选,控件名/角色子串过滤" }
            },
            "required": ["window_id"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let window_id = require_str(&args, "window_id", self.name())?.to_string();
        let max_depth = args
            .get("max_depth")
            .and_then(Value::as_u64)
            .unwrap_or(3)
            .clamp(1, 12) as usize;
        let filter = get_str(&args, "filter").map(str::to_string);
        driver_preflight(self.name())?;
        run_blocking(self.name(), move || {
            let driver = current_driver();
            let tree = driver.inspect(&window_id, max_depth, filter.as_deref())?;
            Ok(tree_to_json(tree))
        })
        .await
    }
}

// ===================== WindowAction =====================

/// 对控件执行操作。
pub struct WindowActionTool;

#[async_trait]
impl Tool for WindowActionTool {
    fn name(&self) -> &str {
        "WindowAction"
    }

    fn description(&self) -> &str {
        "对指定窗口内指定控件执行一个操作。\n\
         - window_id 必填:WindowList 返回的窗口 id。\n\
         - path 必填:WindowInspect 返回的控件路径(如 /0/2/1;\"/\" 表示窗口本身)。\n\
         - action 必填:click(点击按钮等) / invoke(同 click) / focus(聚焦) /\n\
           set_text(写入文本,需 text 参数) / get_text(读取文本) / send_keys(按键,平台支持有限)。\n\
         - text 可选:set_text/send_keys 的文本内容。\n\
         控件是否支持某动作请参考 WindowInspect 返回的 actions 列表;路径失效时重新 WindowInspect。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "window_id": { "type": "string", "description": "WindowList 返回的窗口 id" },
                "path": { "type": "string", "description": "控件路径,如 /0/2/1;\"/\" 表示窗口本身" },
                "action": {
                    "type": "string",
                    "enum": ["click", "invoke", "focus", "set_text", "get_text", "send_keys"],
                    "description": "要执行的动作"
                },
                "text": { "type": "string", "description": "set_text/send_keys 的文本内容" }
            },
            "required": ["window_id", "path", "action"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let window_id = require_str(&args, "window_id", self.name())?.to_string();
        let path = require_str(&args, "path", self.name())?.to_string();
        let action_name = require_str(&args, "action", self.name())?;
        let text = get_str(&args, "text").map(str::to_string);
        let action = ControlAction::parse(action_name, text)?;
        driver_preflight(self.name())?;
        run_blocking(self.name(), move || {
            let driver = current_driver();
            driver.act(&window_id, &path, action)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deep_tree(depth: usize) -> ControlNode {
        let mut node = ControlNode {
            path: "/".into(),
            role: "Window".into(),
            name: "root".into(),
            ..Default::default()
        };
        let mut cur = &mut node;
        for i in 0..depth {
            cur.children.push(ControlNode {
                path: format!("/{i}"),
                role: "Pane".into(),
                name: format!("n{i}"),
                ..Default::default()
            });
            cur = cur.children.last_mut().unwrap();
        }
        node
    }

    #[test]
    fn count_nodes_counts_all() {
        let t = deep_tree(5);
        assert_eq!(count_nodes(&t), 6);
    }

    #[test]
    fn prune_nodes_respects_budget() {
        let mut t = ControlNode {
            path: "/".into(),
            role: "Window".into(),
            name: "root".into(),
            ..Default::default()
        };
        for i in 0..10 {
            t.children.push(ControlNode {
                path: format!("/{i}"),
                role: "Button".into(),
                name: format!("b{i}"),
                ..Default::default()
            });
        }
        let mut budget = 4;
        prune_nodes(&mut t, &mut budget);
        assert_eq!(count_nodes(&t), 5); // root + 4
    }

    #[test]
    fn tree_to_json_truncates_huge_tree() {
        // 构造超过节点上限的树
        let mut root = ControlNode {
            path: "/".into(),
            role: "Window".into(),
            name: "root".into(),
            ..Default::default()
        };
        for i in 0..600 {
            root.children.push(ControlNode {
                path: format!("/{i}"),
                role: "Button".into(),
                name: format!("btn-{i}"),
                ..Default::default()
            });
        }
        let s = tree_to_json(root);
        assert!(s.contains("[truncated]"), "应含截断标注");
    }

    #[tokio::test]
    async fn window_action_validates_params() {
        let t = WindowActionTool;
        // 缺 path
        let err = t
            .execute(json!({"window_id": "1", "action": "click"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("path"));
        // set_text 缺 text
        let err = t
            .execute(json!({"window_id": "1", "path": "/", "action": "set_text"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("text"));
        // 未知 action
        let err = t
            .execute(json!({"window_id": "1", "path": "/", "action": "explode"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("未知 action"));
    }

    #[tokio::test]
    async fn window_inspect_validates_window_id() {
        let t = WindowInspectTool;
        let err = t.execute(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("window_id"));
    }

    #[tokio::test]
    async fn window_find_validates_query() {
        let t = WindowFindTool;
        let err = t.execute(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("query"));
    }

    #[test]
    fn damerau_levenshtein_basic() {
        // 完全相等
        assert_eq!(damerau_levenshtein("abc", "abc"), 0);
        // 单字符插入
        assert_eq!(damerau_levenshtein("abc", "abcd"), 1);
        // 单字符替换
        assert_eq!(damerau_levenshtein("abc", "abd"), 1);
        // 完全不等
        assert!(damerau_levenshtein("abc", "xyz") > 0);
        // 上界剪枝:长度差 > 8 直接返回 max_len("abcdefghijklmnop" = 16 字符)
        let d = damerau_levenshtein("ab", "abcdefghijklmnop");
        assert_eq!(d, 16);
    }

    #[test]
    fn match_mode_parse_aliases() {
        assert_eq!(MatchMode::parse("exact"), MatchMode::Exact);
        assert_eq!(MatchMode::parse("EXACT"), MatchMode::Exact);
        assert_eq!(MatchMode::parse("contains"), MatchMode::Contains);
        assert_eq!(MatchMode::parse("fuzzy"), MatchMode::Fuzzy);
        assert_eq!(MatchMode::parse("unknown"), MatchMode::Contains); // 默认 contains
    }

    fn make_wins() -> Vec<WindowInfo> {
        vec![
            WindowInfo {
                id: "w-1".into(),
                title: "微信(WeChat)".into(),
                process_name: "WeChat".into(),
                pid: 100,
                bounds: Default::default(),
            },
            WindowInfo {
                id: "w-2".into(),
                title: "无标题.txt - 记事本".into(),
                process_name: "Notepad".into(),
                pid: 101,
                bounds: Default::default(),
            },
            WindowInfo {
                id: "w-3".into(),
                title: "Settings".into(),
                process_name: "System Preferences".into(),
                pid: 102,
                bounds: Default::default(),
            },
        ]
    }

    #[test]
    fn pick_top_hit_exact_matches() {
        let wins = make_wins();
        // "wechat" 精确匹配 w-1 的 process_name(WeChat) —— title 是 "微信(WeChat)"
        // 含括号,大小写归一后不等;process 字段命中。
        let hit = pick_top_hit(&wins, "wechat", MatchMode::Exact).unwrap();
        assert_eq!(hit.info.id, "w-1");
        assert_eq!(hit.matched_field, "process");
        assert!((hit.score - 1.0).abs() < 1e-9);
    }

    #[test]
    fn pick_top_hit_contains_substring() {
        let wins = make_wins();
        // "记事本" 是 "无标题.txt - 记事本" 的子串
        let hit = pick_top_hit(&wins, "记事本", MatchMode::Contains).unwrap();
        assert_eq!(hit.info.id, "w-2");
        assert_eq!(hit.matched_field, "title");
    }

    #[test]
    fn pick_top_hit_fuzzy_finds_typo() {
        let wins = make_wins();
        // "WeChqt"(拼错一个字符) 在 contains 模式无命中,在 fuzzy 模式能找到
        assert!(pick_top_hit(&wins, "WeChqt", MatchMode::Contains).is_none());
        let hit = pick_top_hit(&wins, "WeChqt", MatchMode::Fuzzy).unwrap();
        assert_eq!(hit.info.id, "w-1");
        assert!(hit.score > 0.5);
    }

    #[test]
    fn pick_top_hit_no_match_returns_none() {
        let wins = make_wins();
        let q = "完全不存在的应用xyz";
        assert!(pick_top_hit(&wins, q, MatchMode::Contains).is_none());
        assert!(pick_top_hit(&wins, q, MatchMode::Fuzzy).is_none());
        assert!(pick_top_hit(&wins, q, MatchMode::Exact).is_none());
    }

    #[test]
    fn pick_top_hit_picks_higher_score_field() {
        // 测试同 query 在不同字段都有命中
        let wins = vec![
            WindowInfo {
                id: "a".into(),
                title: "abc".into(), // contains "abc"
                process_name: "x".into(),
                pid: 1,
                bounds: Default::default(),
            },
            WindowInfo {
                id: "b".into(),
                title: "x".into(),
                process_name: "abc".into(),
                pid: 2,
                bounds: Default::default(),
            },
        ];
        let hit = pick_top_hit(&wins, "abc", MatchMode::Contains).unwrap();
        assert!(hit.info.id == "a" || hit.info.id == "b");
        assert!(hit.score > 0.3);
    }

    #[test]
    fn default_screenshot_command_macos() {
        if cfg!(target_os = "macos") {
            let cmd = default_screenshot_command("/tmp/x.png", None);
            assert!(cmd.contains("screencapture"));
            assert!(cmd.contains("/tmp/x.png"));
            let cmd2 = default_screenshot_command(
                "/tmp/x.png",
                Some(&json!({"x":10,"y":20,"width":100,"height":200})),
            );
            assert!(cmd2.contains("10,20,100,200"));
        }
    }

    #[test]
    fn default_screenshot_command_linux() {
        if cfg!(not(any(target_os = "macos", windows))) {
            let cmd = default_screenshot_command("/tmp/x.png", None);
            assert!(!cmd.is_empty());
        }
    }
}
