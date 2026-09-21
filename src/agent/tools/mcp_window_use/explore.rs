//! `action=explore`(2026-09-21 第 100 轮):**探索快照**复合 action ——
//! 一次调用完成「capability 探测 + 窗口定位 + 控件树枚举(空树自动 OCR 兜底) +
//! 快照落盘 + actionable 摘要」。
//!
//! 动机(设计见 `docs/MCP_Window_Use/01-设计与解决方案.md` §15):
//!
//! - SubAgent 此前做一次窗口发现要走 `capability_probe → find/open → inspect → ocr`
//!   至少 4~5 次 LLM 往返,每次都重新拼装 tool_call 参数 + 截断的控件树让人机
//!   共用机器时 Agent 动作太慢、反复抢焦点;
//! - 本 action 把这四步压缩为一次调用,把全量控件树落盘到 snapshot_path
//!   (LLM 后续可 Read 工具按需读,不受内联 32KB 截断限制),返回紧凑的
//!   actionable 摘要(控件 path / role / name / bounds / screen_cx / screen_cy)
//!   供下一步 `run_sequence` 直接拼装 steps;
//! - 与第 91 轮 run_sequence 配合形成「**先 explore 后 run_sequence**」的连续
//!   执行模式(单步 vs 连续 双模式体系正式落地);后续可在 step.path 使用
//!   `@explore_ref/<snapshot_id>/...` 约定引用本快照中的节点。
//!
//! 护栏:`max_depth` ∈ [1, 12](默认 6);snapshot_path 缺省 = `<工作目录>/
//! laew_ui_snapshot_<unix_ts>.json`;digest 序列化目标 ≤ 80KB;
//! 写盘 tree_full 不截断(由 LLM 后续 Read 按需加载);Doom Loop 检测在 mod.rs 中,
//! 本模块不重复实现。
use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::{json, Value};

use super::*;
use crate::agent::window::{
    auto_launch_target, probe_capability, OcrBlock, WindowCapability, WindowInfo,
};

/// 默认 max_depth(连续模式放宽,探索更彻底)。
const DEFAULT_MAX_DEPTH: u64 = 6;
/// max_depth 上限。
const MAX_DEPTH_CAP: u64 = 12;
/// 快照默认前缀。
const SNAPSHOT_PREFIX: &str = "laew_ui_snapshot_";
/// digest 序列化软目标(超过则提示精简 max_depth/filter,不限流)。
const DIGEST_TARGET_BYTES: usize = 80 * 1024;
/// 进程级 snapshot 缓存(snapshot_id → 全量 tree 节点引用)。
///
/// 用途:run_sequence 步骤里 path 以 `@explore_ref/<snapshot_id>/...` 开头时,
/// 本缓存命中则跳过驱动 inspect(零 RTT);miss 则按字面 path 走驱动。
pub(super) fn snapshot_cache() -> &'static std::sync::Mutex<HashMap<String, ControlNode>> {
    static CACHE: OnceLock<std::sync::Mutex<HashMap<String, ControlNode>>> = OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// `action=explore` 入口:参数校验 + 同步执行循环(`run_blocking` 包裹)。
pub(super) async fn run_explore(args: Value) -> Result<String> {
    // 参数校验:query 与 window_id 二选一
    let query = get_str(&args, "query").map(str::to_string);
    let explicit_window_id = get_str(&args, "window_id").map(str::to_string);
    if query.is_none() && explicit_window_id.is_none() {
        return Err(tool_err(
            MCP_WINDOW_USE_TOOL_NAME,
            "explore 必填参数 query(窗口查询词) 或 window_id(已知窗口 id) 二选一",
        ));
    }
    let max_depth = args
        .get("max_depth")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MAX_DEPTH)
        .clamp(1, MAX_DEPTH_CAP);
    let filter = get_str(&args, "filter").map(str::to_string);
    let include_ocr = args
        .get("include_ocr")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let snapshot_label = get_str(&args, "snapshot_label").map(str::to_string);
    let wait_seconds = args
        .get("wait_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(10)
        .min(30);
    let snapshot_path_override = get_str(&args, "snapshot_path").map(str::to_string);

    let app_name = get_str(&args, "app_name").map(str::to_string);
    let bundle_id = get_str(&args, "bundle_id").map(str::to_string);

    run_blocking(MCP_WINDOW_USE_TOOL_NAME, move || {
        let driver = current_driver();

        // 1. capability 探测(走既有缓存,无 RTT 开销)
        let capability = probe_capability();
        let recommended_route = capability.next_action_hint();

        // 2. 解析 window_id:query 模式走 find / open 双路径
        let (window_id, window_info) = if let Some(wid) = explicit_window_id {
            // window_id 模式:list_windows 拿完整 WindowInfo 以便后续 OCR/坐标换算
            let wins = driver.list_windows(None)?;
            let info = wins
                .into_iter()
                .find(|w| w.id == wid)
                .ok_or_else(|| {
                    tool_err(
                        MCP_WINDOW_USE_TOOL_NAME,
                        format!("window_id={wid:?} 不在当前可见窗口列表;请重新 action=list"),
                    )
                })?;
            (wid, info)
        } else {
            // query 模式:复用 find 逻辑(包含中英文别名);找不到则尝试 open 启动
            let q = query.expect("query 与 window_id 已校验二选一");
            let aliases = expand_window_query(&q);
            let wins = driver.list_windows(None)?;
            let hit = find_window_by_aliases(&wins, &aliases);
            if let Some((_, info)) = hit {
                (info.id.clone(), info)
            } else {
                // 启动:委托 open.rs 的 launch 路径
                drop(wins);
                let launched = launch_for_explore(&q, app_name.as_deref(), bundle_id.as_deref(), wait_seconds)?;
                (launched.id.clone(), launched)
            }
        };

        // 3. 校验窗口仍存在 + 可选前台(避免后续 inspect 对已最小化窗口返回空)
        let _ = driver.bring_to_front(&window_id);

        // 4. inspect 控件树
        driver_preflight_sync()?;
        let tree = driver.inspect(&window_id, max_depth as usize, filter.as_deref())?;
        let total_nodes = count_nodes(&tree);
        let actionable = collect_actionable(&tree);
        let self_drawn = actionable.is_empty();

        // 5. OCR 兜底(自绘 UI + 屏录可用时)
        let mut ocr_blocks: Vec<OcrBlock> = Vec::new();
        let mut ocr_used = false;
        if self_drawn && include_ocr && capability.ocr_screenshot_cgwindow {
            match driver.ocr_with_info(&window_info, None, None) {
                Ok(blocks) => {
                    ocr_blocks = blocks;
                    ocr_used = true;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "explore OCR 兜底失败,继续走控件树路线");
                }
            }
        }

        // 6. 构造 snapshot_id + 写盘
        let snapshot_id = random_snap_id();
        let snapshot_path = resolve_snapshot_path(snapshot_path_override.as_deref());
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let snapshot_json = json!({
            "snapshot_id": snapshot_id,
            "snapshot_label": snapshot_label,
            "created_at": created_at,
            "window_info": window_info,
            "capability": capability_as_value(&capability),
            "tree_full": tree,
            "ocr_blocks": ocr_blocks,
        });
        let snapshot_text = serde_json::to_string_pretty(&snapshot_json).map_err(|e| {
            tool_err(
                MCP_WINDOW_USE_TOOL_NAME,
                format!("snapshot 序列化失败: {e}"),
            )
        })?;
        std::fs::write(&snapshot_path, snapshot_text.as_bytes()).map_err(|e| {
            tool_err(
                MCP_WINDOW_USE_TOOL_NAME,
                format!("snapshot 写盘失败: path={snapshot_path:?} err={e}"),
            )
        })?;

        // 7. 缓存全量 tree(供后续 @explore_ref 引用)
        if let Ok(mut cache) = snapshot_cache().lock() {
            // 同 id 不会冲突(随机生成);旧条目淘汰策略:超过 16 条丢最早。
            if cache.len() >= 16 {
                if let Some(oldest) = cache.keys().next().cloned() {
                    cache.remove(&oldest);
                }
            }
            cache.insert(snapshot_id.clone(), tree.clone());
        }

        // 8. 构造 actionable digest(带屏幕绝对坐标中心)
        let actionable_list = build_actionable_digest(&actionable, &window_info, &ocr_blocks);
        let tree_summary = json!({
            "total_nodes": total_nodes,
            "actionable_count": actionable.len(),
            "depth_reached": max_depth,
            "truncated": total_nodes >= MAX_TREE_NODES,
            "self_drawn": self_drawn,
            "ocr_used": ocr_used,
        });

        let mut body = json!({
            "ok": true,
            "action": "explore",
            "snapshot_id": snapshot_id,
            "snapshot_path": snapshot_path,
            "snapshot_label": snapshot_label,
            "window_info": window_info,
            "capability": capability_as_value(&capability),
            "recommended_route": recommended_route,
            "tree_summary": tree_summary,
            "actionable": actionable_list,
        });
        if !ocr_blocks.is_empty() {
            body["ocr_blocks"] = json!(ocr_blocks_to_value(&ocr_blocks, &window_info));
        }
        body["next_action"] = json!(format!(
            "snapshot 已落盘(可 Read 加载细节);连续执行请用 action=run_sequence(steps=[...]),\
             步骤中 path 直接引用 actionable[*].path 或坐标取 actionable[*].screen_cx/screen_cy;\
             snapshot_id={snapshot_id} 节点亦可用 @explore_ref/{snapshot_id}/<path> 引用(同会话缓存命中,零 RTT)"
        ));

        let body_text = serde_json::to_string_pretty(&body).map_err(|e| {
            tool_err(
                MCP_WINDOW_USE_TOOL_NAME,
                format!("explore 返回体序列化失败: {e}"),
            )
        })?;
        // digest 软目标告警(不阻断)
        if body_text.len() > DIGEST_TARGET_BYTES {
            eprintln!(
                "  [explore] ⚠ digest {} 字节超过软目标 {}KB,可加大 filter 或减小 max_depth 精简",
                body_text.len(),
                DIGEST_TARGET_BYTES / 1024
            );
        }
        Ok(body_text)
    })
    .await
}

/// 探索阶段无前置权限对话框(避免给用户带来多重弹窗);inspect 失败仍由
/// 调用方感知。
fn driver_preflight_sync() -> Result<()> {
    let driver = current_driver();
    if let Some(hint) = driver.permission_hint() {
        return Err(tool_err(MCP_WINDOW_USE_TOOL_NAME, hint));
    }
    Ok(())
}

/// query 模式启动目标应用 —— 复用 auto_launch_target(平台原生启动 + 等待窗口出现)。
fn launch_for_explore(
    query: &str,
    app_name: Option<&str>,
    bundle_id: Option<&str>,
    wait_seconds: u64,
) -> Result<WindowInfo> {
    let app = app_name.unwrap_or(query);
    auto_launch_target(app, bundle_id, wait_seconds).map_err(|e| {
        tool_err(
            MCP_WINDOW_USE_TOOL_NAME,
            format!("explore 启动目标应用 {query:?} 失败: {e}"),
        )
    })
}

/// 递归收集所有可操作控件(走 inspect.rs 的 has_actionable_controls 同款角色表)。
fn collect_actionable(root: &ControlNode) -> Vec<ControlNode> {
    fn walk(n: &ControlNode, out: &mut Vec<ControlNode>) {
        if !n.role.is_empty() && is_actionable_role(&n.role) {
            out.push(n.clone());
        }
        for c in &n.children {
            walk(c, out);
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

fn is_actionable_role(role: &str) -> bool {
    const ROLES: &[&str] = &[
        "button", "edit", "text", "list", "listitem", "menuitem", "menu",
        "checkbox", "radiobutton", "combo", "slider", "tab", "treeitem",
        "tree", "hyperlink", "link", "dataitem", "custom", "row", "cell",
        "outline", "image", "scrollbar",
    ];
    let r = role.trim().to_ascii_lowercase();
    let r = r.strip_prefix("ax").unwrap_or(&r);
    ROLES.iter().any(|k| r.contains(k))
}

/// actionable 列表 → digest 数组(含屏幕绝对坐标中心 + 可用 actions)。
fn build_actionable_digest(
    actionable: &[ControlNode],
    window_info: &WindowInfo,
    ocr_blocks: &[OcrBlock],
) -> Vec<Value> {
    let wx = window_info.bounds.x;
    let wy = window_info.bounds.y;
    let mut out = Vec::with_capacity(actionable.len());
    for (i, n) in actionable.iter().enumerate() {
        // 控件 bounds 中心(屏幕绝对) = WindowInfo.bounds.origin + ControlNode.bounds.center
        let cx = wx + n.bounds.x + n.bounds.width / 2;
        let cy = wy + n.bounds.y + n.bounds.height / 2;
        // 文本对齐到 OCR 词块(若控件 name 出现在 OCR 中则取更精确坐标)
        let (cx, cy) = ocr_aligned_center(ocr_blocks, wx, wy, &n.name)
            .unwrap_or((cx, cy));
        out.push(json!({
            "i": i,
            "path": n.path,
            "role": n.role,
            "name": n.name,
            "value": n.value,
            "bounds": n.bounds,
            "screen_cx": cx,
            "screen_cy": cy,
            "actions": n.actions,
        }));
    }
    out
}

/// 若控件 name 与某 OCR 词块文本匹配,采用 OCR 的屏幕绝对坐标(更精确)。
fn ocr_aligned_center(
    ocr_blocks: &[OcrBlock],
    window_x: i64,
    window_y: i64,
    name: &str,
) -> Option<(i64, i64)> {
    if name.is_empty() {
        return None;
    }
    for b in ocr_blocks {
        if b.text.contains(name) || name.contains(&b.text) {
            let cx = window_x + b.x + b.width / 2;
            let cy = window_y + b.y + b.height / 2;
            return Some((cx, cy));
        }
    }
    None
}

/// OCR 词块 → JSON(含屏幕绝对坐标)。
fn ocr_blocks_to_value(blocks: &[OcrBlock], window_info: &WindowInfo) -> Vec<Value> {
    let wx = window_info.bounds.x;
    let wy = window_info.bounds.y;
    blocks
        .iter()
        .map(|b| {
            json!({
                "text": b.text,
                "x": b.x, "y": b.y, "width": b.width, "height": b.height,
                "screen_cx": wx + b.x + b.width / 2,
                "screen_cy": wy + b.y + b.height / 2,
            })
        })
        .collect()
}

fn capability_as_value(c: &WindowCapability) -> Value {
    json!({
        "list_find": c.list_find,
        "inspect_control": c.inspect_control,
        "ocr_screenshot_cgwindow": c.ocr_screenshot_cgwindow,
        "coordinate_input": c.coordinate_input,
        "screencapture_cli": c.screencapture_cli,
        "ax_warmup": c.ax_warmup,
        "tag": c.tag(),
    })
}

fn resolve_snapshot_path(override_path: Option<&str>) -> String {
    if let Some(p) = override_path.filter(|s| !s.is_empty()) {
        return p.to_string();
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    cwd.join(format!("{SNAPSHOT_PREFIX}{ts}.json"))
        .to_string_lossy()
        .to_string()
}

fn random_snap_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    // 6 位 base36 后缀,碰撞概率 ~1e-9/任务
    let mut v = nanos.wrapping_mul(2654435761) & 0xFFFFFFFF;
    let mut out = String::with_capacity(6);
    for _ in 0..6 {
        let d = (v % 36) as u32;
        let c = if d < 10 {
            (b'0' + d as u8) as char
        } else {
            (b'a' + (d - 10) as u8) as char
        };
        out.push(c);
        v /= 36;
    }
    out
}

/// 解析 `@explore_ref/<snapshot_id>/<path>` 引用:命中缓存返回 true + 实际 path;
/// 未命中返回 false(调用方按字面 path 处理)。
///
/// 返回 `(matched_cache, literal_path)`:matched_cache=true 表示本调用方应
/// 跳过驱动调用,直接消费缓存(目前仅用于占位,实际 run_sequence/input_batch
/// 仍按字面 path 走;本函数保留为后续优化入口)。
pub(super) fn resolve_explore_ref(path: &str) -> (bool, String) {
    const PREFIX: &str = "@explore_ref/";
    if !path.starts_with(PREFIX) {
        return (false, path.to_string());
    }
    let rest = &path[PREFIX.len()..];
    if let Some(slash) = rest.find('/') {
        let id = &rest[..slash];
        let real = &rest[slash + 1..];
        if real.is_empty() {
            return (false, format!("/")); // 根窗口
        }
        let real_path = if real.starts_with('/') {
            real.to_string()
        } else {
            format!("/{real}")
        };
        if let Ok(cache) = snapshot_cache().lock() {
            if cache.contains_key(id) {
                return (true, real_path);
            }
        }
        return (false, real_path);
    }
    (false, "/".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::window::{ControlNode, Rect};

    fn btn(path: &str, name: &str, x: i64, y: i64, w: i64, h: i64) -> ControlNode {
        ControlNode {
            path: path.into(),
            role: "Button".into(),
            name: name.into(),
            bounds: Rect {
                x,
                y,
                width: w,
                height: h,
            },
            actions: vec!["click".into(), "invoke".into()],
            ..Default::default()
        }
    }

    #[test]
    fn collect_actionable_filters_pane_and_keeps_buttons() {
        let mut root = ControlNode {
            path: "/".into(),
            role: "Window".into(),
            name: "root".into(),
            children: vec![
                btn("/0", "发送", 10, 20, 80, 30),
                ControlNode {
                    path: "/1".into(),
                    role: "Pane".into(),
                    name: "面板".into(),
                    children: vec![btn("/1/0", "确定", 100, 200, 60, 25)],
                    ..Default::default()
                },
                ControlNode {
                    path: "/2".into(),
                    role: "AXButton".into(),
                    name: "ax按钮".into(),
                    children: vec![],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        // 修复空切:ControlNode 已 .. Default, 不需要 children: vec![]
        root.children[0].children.clear();
        let mut v = collect_actionable(&root);
        v.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].path, "/0");
        assert_eq!(v[1].path, "/1/0");
        assert_eq!(v[2].path, "/2"); // AXButton 剥离 AX 前缀命中
        assert_eq!(v[2].name, "ax按钮");
    }

    #[test]
    fn build_actionable_digest_uses_window_origin() {
        let buttons = vec![btn("/0", "发送", 10, 20, 80, 30)];
        let info = WindowInfo::basic(
            "w".into(),
            "微信".into(),
            "WeChat".into(),
            100,
            Rect {
                x: 200,
                y: 50,
                width: 800,
                height: 600,
            },
        );
        let v = build_actionable_digest(&buttons, &info, &[]);
        assert_eq!(v.len(), 1);
        // 控件 bounds = (10,20,80,30), 中心 (50,35) → 屏幕 (200+50, 50+35) = (250, 85)
        assert_eq!(v[0]["screen_cx"], json!(250));
        assert_eq!(v[0]["screen_cy"], json!(85));
        assert_eq!(v[0]["path"], json!("/0"));
    }

    #[test]
    fn resolve_snapshot_path_default_uses_cwd_and_ts() {
        let p = resolve_snapshot_path(None);
        assert!(p.contains(SNAPSHOT_PREFIX), "{p}");
        assert!(p.ends_with(".json"), "{p}");
    }

    #[test]
    fn resolve_snapshot_path_respects_override() {
        let p = resolve_snapshot_path(Some("/tmp/custom_snapshot.json"));
        assert_eq!(p, "/tmp/custom_snapshot.json");
    }

    #[test]
    fn random_snap_id_is_six_chars() {
        let id = random_snap_id();
        assert_eq!(id.len(), 6);
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn resolve_explore_ref_parses_prefix() {
        // 命中缓存
        {
            let mut cache = snapshot_cache().lock().unwrap();
            cache.insert("a1b2c3".into(), btn("/x", "OK", 0, 0, 10, 10));
        }
        let (hit, p) = resolve_explore_ref("@explore_ref/a1b2c3/0/2/1");
        assert!(hit);
        assert_eq!(p, "/0/2/1");
        // miss(id 不在缓存)
        let (hit, p) = resolve_explore_ref("@explore_ref/zzz999/0/2/1");
        assert!(!hit);
        assert_eq!(p, "/0/2/1");
        // 不带前缀(未来打开探索也会按字面处理)
        let (hit, p) = resolve_explore_ref("/0/2/1");
        assert!(!hit);
        assert_eq!(p, "/0/2/1");
        // 仅 snapshot_id 无 path → 回退根
        let (hit, p) = resolve_explore_ref("@explore_ref/a1b2c3");
        assert!(!hit);
        assert_eq!(p, "/");
    }

    #[test]
    fn ocr_aligned_center_prefers_ocr_match() {
        let blocks = vec![OcrBlock {
            text: "发送".into(),
            x: 50,
            y: 80,
            width: 40,
            height: 20,
        }];
        // name 与 OCR 词块完全匹配 → 取 OCR 中心(屏幕 = window_x + 50+20, window_y + 80+10)
        let (cx, cy) = ocr_aligned_center(&blocks, 200, 50, "发送").unwrap();
        assert_eq!(cx, 270);
        assert_eq!(cy, 140);
        // 不匹配 → None
        assert!(ocr_aligned_center(&blocks, 0, 0, "无关文本").is_none());
        // name 为空 → None
        assert!(ocr_aligned_center(&blocks, 0, 0, "").is_none());
    }
}