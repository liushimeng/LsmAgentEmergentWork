//! 控件操作域模型(2026-09-19 第 90 轮自 window/mod.rs 机械拆分,遵守单文件
//! ≤1800 行规范;`window/mod.rs` 已达 1700+ 临界线)。
//!
//! 内容与拆分前逐行一致(仅新增本模块文档头与 use):
//! - [`ControlAction`] 枚举:控件树路线(click/set_text/get_text/...)与
//!   视觉坐标路线(click_point/drag_point/...)全部动作变体,含第 90 轮新增的
//!   `MovePoint` / `MiddleClickPoint` / `DragPoint` 与点击系 `modifiers` 修饰键;
//! - `parse` / `parse_ext` 动作名解析(大小写不敏感、连字符/下划线归一、
//!   坐标与修饰键参数校验);
//! - `parse_scroll_lines` 滚动方向/行数解析;
//! - 对应单元测试 4 组(parse 别名/scroll/type_text_submit/第 90 轮点原语)。

use crate::error::{AgentError, Result};

/// 控件操作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlAction {
    /// 点击(UIA Invoke / AXPress / Win32 BM_CLICK)
    Click,
    /// 聚焦
    Focus,
    /// 写入文本(Value Pattern / AXValue / WM_SETTEXT)
    SetText(String),
    /// 读取文本(无副作用)
    GetText,
    /// 发送按键(尽力而为,平台差异大)
    SendKeys(String),
    /// 调用默认动作(等价 Click,保留给 UIA Invoke 语义明确的场景)
    Invoke,
    /// 滚动(2026-09-16 第 66 轮):lines>0 向上滚,lines<0 向下滚,单位为「行」。
    /// macOS 走 CGEvent 滚轮事件(光标先移到控件中心);Windows 走 WM_MOUSEWHEEL;
    /// fallback 走 xdotool click 4/5。
    Scroll {
        /// 滚动行数(正=上,负=下)
        lines: i32,
    },
    /// 把目标控件滚动到可见区域(2026-09-16 第 66 轮):
    /// macOS AXScrollToVisible;Windows 暂等价 Scroll 小步;列表逐条定位场景比盲滚精准。
    ScrollToVisible,
    /// ===== 2026-09-16 第 67 轮:坐标动作(自绘 UI 视觉路线) =====
    /// 坐标信息来自 `MCP_Window_Use(action=ocr)` 返回的词块(screen_x/screen_y 取中心)或窗口 bounds 计算。
    /// 在屏幕绝对坐标 (x,y) 执行物理鼠标左键单击(SendInput / CGEvent / xdotool)。
    /// 2026-09-19 第 90 轮:`modifiers` 可选修饰键规格("ctrl" / "ctrl+shift"),
    /// 按住修饰键再点击(ctrl+点击多选 / shift+点击区选),与 send_keys 组合键语法同源。
    ClickPoint {
        x: i64,
        y: i64,
        modifiers: Option<String>,
    },
    /// 坐标双击(展开列表项 / 打开会话等场景)。`modifiers` 语义同 [`ControlAction::ClickPoint`]。
    DoubleClickPoint {
        x: i64,
        y: i64,
        modifiers: Option<String>,
    },
    /// 坐标右键(呼出上下文菜单)。`modifiers` 语义同 [`ControlAction::ClickPoint`]。
    RightClickPoint {
        x: i64,
        y: i64,
        modifiers: Option<String>,
    },
    /// 在 (x,y) 处滚动滚轮(先移光标再滚,自绘 UI 对消息滚动不敏感,物理事件最可靠)。
    ScrollPoint { x: i64, y: i64, lines: i32 },
    /// ===== 2026-09-19 第 90 轮:鼠标键盘原子能力(同时操作鼠标、键盘) =====
    /// 仅移动光标到 (x,y),不点击(悬停触发菜单展开 / tooltip / 列表预览)。
    MovePoint { x: i64, y: i64 },
    /// 坐标中键点击(浏览器链接新标签打开 / 某些 CAD 平移视图)。
    MiddleClickPoint { x: i64, y: i64 },
    /// 从 (x,y) 按住左键拖拽到 (x2,y2)(文件拖动 / 滑块 / 选区);
    /// `modifiers` 可选修饰键(ctrl+拖=复制等),拖拽期间按住 —— 键盘与鼠标同时操作。
    /// 拖拽无 T1/T2 路径(UIA DragPattern 极少实现,Win32 无通用内容拖拽消息),
    /// 跨平台恒走 T3 物理输入。
    DragPoint {
        x: i64,
        y: i64,
        x2: i64,
        y2: i64,
        modifiers: Option<String>,
    },
    /// 向**当前焦点控件**真实键入文本(SendInput Unicode / CGEvent keystroke)。
    /// 配合 `click_point` 先点输入框使用;自绘输入框(微信 4.x 等)唯一可靠的输入路径。
    TypeText(String),
    /// 键入完整文本后立即提交(2026-09-17 第 80 轮)。
    ///
    /// 聊天发送框、搜索框、命令面板等场景中,`type_text` 与 `send_keys(enter)`
    /// 拆成两次工具调用会引入 LLM 返场和焦点迁移窗口;本动作在驱动层原子完成
    /// “可选点击定位 → 键入 → 等待应用消费 → Enter”。`x/y` 为可选屏幕绝对坐标。
    TypeTextSubmit {
        text: String,
        x: Option<i64>,
        y: Option<i64>,
    },
}

impl ControlAction {
    /// 从工具参数解析动作名(大小写不敏感,连字符/下划线归一)。
    pub fn parse(name: &str, text: Option<String>) -> Result<Self> {
        Self::parse_ext(name, text, None, None, None, None, None)
    }

    /// 2026-09-16 第 67 轮:带坐标参数的动作解析(x/y 供 click_point / scroll_point 系使用)。
    /// 2026-09-19 第 90 轮:扩展 x2/y2(drag_point 终点)与 modifiers(修饰键规格,
    /// 作用于 click_point 系 / drag_point,实现「按住键盘修饰键 + 鼠标操作」)。
    pub fn parse_ext(
        name: &str,
        text: Option<String>,
        x: Option<i64>,
        y: Option<i64>,
        x2: Option<i64>,
        y2: Option<i64>,
        modifiers: Option<String>,
    ) -> Result<Self> {
        let norm = name.trim().to_lowercase().replace(['-', '_'], "");
        let modifiers = modifiers
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty());
        if let Some(m) = &modifiers {
            // 提前校验修饰键规格(仅允许 ctrl/shift/alt/win(+组合),防拼错落到驱动层才失败)
            for seg in m.split('+').map(str::trim).filter(|s| !s.is_empty()) {
                let ok = matches!(
                    seg.to_lowercase().as_str(),
                    "ctrl" | "control" | "shift" | "alt" | "option" | "opt" | "win" | "meta" | "cmd" | "command"
                );
                if !ok {
                    return Err(AgentError::ToolExecution {
                        tool: "MCP_Window_Use(action=control)".into(),
                        reason: format!(
                            "modifiers 段无法识别: {seg:?}(仅允许 ctrl/shift/alt/win(+组合),如 \"ctrl\" / \"ctrl+shift\")"
                        ),
                    });
                }
            }
        }
        // 坐标类动作统一校验 x/y 必填(坐标动作不接受 path 定位,path 照传 "/")
        let need_point = || -> std::result::Result<(i64, i64), AgentError> {
            match (x, y) {
                (Some(px), Some(py)) => Ok((px, py)),
                _ => Err(AgentError::ToolExecution {
                    tool: "MCP_Window_Use(action=control)".into(),
                    reason: format!(
                        "action={norm} 缺少整数参数 x / y(屏幕绝对坐标,取 MCP_Window_Use(action=ocr) 返回的 screen_x/screen_y 中心)"
                    ),
                }),
            }
        };
        Ok(match norm.as_str() {
            "click" => Self::Click,
            "focus" => Self::Focus,
            "settext" | "input" | "type" => {
                Self::SetText(text.ok_or_else(|| AgentError::ToolExecution {
                    tool: "MCP_Window_Use(action=control)".into(),
                    reason: "action=set_text 缺少 string 类型参数 text".into(),
                })?)
            }
            "gettext" | "read" => Self::GetText,
            "sendkeys" | "keys" => {
                Self::SendKeys(text.ok_or_else(|| AgentError::ToolExecution {
                    tool: "MCP_Window_Use(action=control)".into(),
                    reason: "action=send_keys 缺少 string 类型参数 text".into(),
                })?)
            }
            "invoke" | "press" => Self::Invoke,
            // 2026-09-16 第 66 轮:scroll 动作,text 形如 "down:3" / "up:5" / "down"(缺省 3 行)。
            "scroll" => Self::Scroll {
                lines: parse_scroll_lines(text.as_deref())?,
            },
            "scrolltovisible" | "scrollintoview" | "reveal" => Self::ScrollToVisible,
            // ===== 第 67 轮:坐标动作 =====
            "clickpoint" | "pointclick" | "clickat" => {
                let (px, py) = need_point()?;
                Self::ClickPoint {
                    x: px,
                    y: py,
                    modifiers: modifiers.clone(),
                }
            }
            "doubleclickpoint" | "pointdoubleclick" | "doubleclickat" => {
                let (px, py) = need_point()?;
                Self::DoubleClickPoint {
                    x: px,
                    y: py,
                    modifiers: modifiers.clone(),
                }
            }
            "rightclickpoint" | "pointrightclick" | "rightclickat" => {
                let (px, py) = need_point()?;
                Self::RightClickPoint {
                    x: px,
                    y: py,
                    modifiers: modifiers.clone(),
                }
            }
            "scrollpoint" | "pointscroll" | "scrollat" => {
                let (px, py) = need_point()?;
                Self::ScrollPoint {
                    x: px,
                    y: py,
                    lines: parse_scroll_lines(text.as_deref())?,
                }
            }
            "typetext" | "inputatfocus" | "typeatfocus" => {
                Self::TypeText(text.ok_or_else(|| AgentError::ToolExecution {
                    tool: "MCP_Window_Use(action=control)".into(),
                    reason: "action=type_text 缺少 string 类型参数 text".into(),
                })?)
            }
            "typetextsubmit" | "sendtext" | "typeandsubmit" => {
                let text = text.ok_or_else(|| AgentError::ToolExecution {
                    tool: "MCP_Window_Use(action=control)".into(),
                    reason: "action=type_text_submit 缺少 string 类型参数 text".into(),
                })?;
                Self::TypeTextSubmit {
                    text,
                    x: x.filter(|v| *v > 0),
                    y: y.filter(|v| *v > 0),
                }
            }
            // ===== 第 90 轮:鼠标键盘原子能力(悬停 / 中键 / 拖拽) =====
            "movepoint" | "pointmove" | "movecursor" | "hover" | "mousemove" => {
                let (px, py) = need_point()?;
                Self::MovePoint { x: px, y: py }
            }
            "middleclickpoint" | "pointmiddleclick" | "middleclickat" => {
                let (px, py) = need_point()?;
                Self::MiddleClickPoint { x: px, y: py }
            }
            "dragpoint" | "pointdrag" | "dragat" | "drag" => {
                let (px, py) = need_point()?;
                let (px2, py2) = match (x2, y2) {
                    (Some(a), Some(b)) => (a, b),
                    _ => {
                        return Err(AgentError::ToolExecution {
                            tool: "MCP_Window_Use(action=control)".into(),
                            reason: format!(
                                "action={norm} 缺少整数参数 x2 / y2(拖拽终点屏幕绝对坐标;x/y 是起点)"
                            ),
                        })
                    }
                };
                Self::DragPoint {
                    x: px,
                    y: py,
                    x2: px2,
                    y2: py2,
                    modifiers: modifiers.clone(),
                }
            }
            other => {
                return Err(AgentError::ToolExecution {
                    tool: "MCP_Window_Use(action=control)".into(),
                    reason: format!(
                        "未知 action: {other};可用: click / focus / set_text / get_text / send_keys / invoke / scroll / scroll_to_visible / click_point / double_click_point / right_click_point / scroll_point / type_text / type_text_submit / move_point / middle_click_point / drag_point"
                    ),
                })
            }
        })
    }

    /// 是否为屏幕坐标类动作(工具层据此提示 LLM 坐标来源)。
    /// 第 90 轮:覆盖 move_point / middle_click_point / drag_point 新变体。
    pub fn is_point_action(&self) -> bool {
        matches!(
            self,
            Self::ClickPoint { .. }
                | Self::DoubleClickPoint { .. }
                | Self::RightClickPoint { .. }
                | Self::ScrollPoint { .. }
                | Self::MovePoint { .. }
                | Self::MiddleClickPoint { .. }
                | Self::DragPoint { .. }
        )
    }
}

/// 解析 scroll 的方向与行数:"down:3" / "up:5" / "down"(缺省 3 行)。
/// 返回 lines:正=向上,负=向下(对齐滚轮物理语义)。
fn parse_scroll_lines(text: Option<&str>) -> Result<i32> {
    let raw = text.unwrap_or("down").trim().to_lowercase();
    let (dir, num_part) = match raw.split_once(':') {
        Some((d, n)) => (d.trim(), n.trim()),
        None => (raw.as_str(), ""),
    };
    let magnitude: i32 = if num_part.is_empty() {
        3
    } else {
        num_part.parse().map_err(|_| AgentError::ToolExecution {
            tool: "MCP_Window_Use(action=control)".into(),
            reason: format!("scroll 行数非法: {num_part}(应为正整数,如 \"down:3\")"),
        })?
    };
    if magnitude <= 0 || magnitude > 100 {
        return Err(AgentError::ToolExecution {
            tool: "MCP_Window_Use(action=control)".into(),
            reason: format!("scroll 行数超出范围(1-100): {magnitude}"),
        });
    }
    match dir {
        "up" | "upward" | "上" => Ok(magnitude),
        "down" | "downward" | "下" => Ok(-magnitude),
        other => Err(AgentError::ToolExecution {
            tool: "MCP_Window_Use(action=control)".into(),
            reason: format!("scroll 方向非法: {other}(应为 up/down)"),
        }),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_action_parse_aliases() {
        assert_eq!(
            ControlAction::parse("click", None).unwrap(),
            ControlAction::Click
        );
        assert_eq!(
            ControlAction::parse("CLICK", None).unwrap(),
            ControlAction::Click
        );
        assert_eq!(
            ControlAction::parse("focus", None).unwrap(),
            ControlAction::Focus
        );
        assert_eq!(
            ControlAction::parse("set_text", Some("hi".into())).unwrap(),
            ControlAction::SetText("hi".into())
        );
        assert_eq!(
            ControlAction::parse("input", Some("x".into())).unwrap(),
            ControlAction::SetText("x".into())
        );
        assert_eq!(
            ControlAction::parse("get-text", None).unwrap(),
            ControlAction::GetText
        );
        assert_eq!(
            ControlAction::parse("send_keys", Some("Enter".into())).unwrap(),
            ControlAction::SendKeys("Enter".into())
        );
        assert_eq!(
            ControlAction::parse("invoke", None).unwrap(),
            ControlAction::Invoke
        );
        assert!(ControlAction::parse("set_text", None).is_err());
        assert!(ControlAction::parse("send_keys", None).is_err());
        assert!(ControlAction::parse("explode", None).is_err());
    }

    #[test]
    fn control_action_parse_scroll() {
        // 2026-09-16 第 66 轮:scroll 动作解析(方向 + 行数,正=上,负=下)
        assert_eq!(
            ControlAction::parse("scroll", Some("down:3".into())).unwrap(),
            ControlAction::Scroll { lines: -3 }
        );
        assert_eq!(
            ControlAction::parse("scroll", Some("up:5".into())).unwrap(),
            ControlAction::Scroll { lines: 5 }
        );
        // 缺省 3 行向下
        assert_eq!(
            ControlAction::parse("scroll", None).unwrap(),
            ControlAction::Scroll { lines: -3 }
        );
        assert_eq!(
            ControlAction::parse("scroll", Some("down".into())).unwrap(),
            ControlAction::Scroll { lines: -3 }
        );
        // 中文方向别名
        assert_eq!(
            ControlAction::parse("scroll", Some("上:2".into())).unwrap(),
            ControlAction::Scroll { lines: 2 }
        );
        // scroll_to_visible 别名
        assert_eq!(
            ControlAction::parse("scroll_to_visible", None).unwrap(),
            ControlAction::ScrollToVisible
        );
        assert_eq!(
            ControlAction::parse("scroll-into-view", None).unwrap(),
            ControlAction::ScrollToVisible
        );
        // 非法:方向未知 / 行数非数 / 超范围
        assert!(ControlAction::parse("scroll", Some("left:3".into())).is_err());
        assert!(ControlAction::parse("scroll", Some("down:abc".into())).is_err());
        assert!(ControlAction::parse("scroll", Some("down:0".into())).is_err());
        assert!(ControlAction::parse("scroll", Some("down:101".into())).is_err());
    }

    #[test]
    fn control_action_parse_type_text_submit() {
        // 2026-09-17 第 80 轮:原子发送动作是跨平台工具契约。
        // 第 90 轮:parse_ext 扩展为 7 参(x2/y2/modifiers),调用点同步更新。
        assert_eq!(
            ControlAction::parse_ext(
                "type_text_submit",
                Some("hello".into()),
                None,
                None,
                None,
                None,
                None
            )
            .unwrap(),
            ControlAction::TypeTextSubmit {
                text: "hello".into(),
                x: None,
                y: None
            }
        );
        assert_eq!(
            ControlAction::parse_ext(
                "sendtext",
                Some("发送".into()),
                Some(120),
                Some(240),
                None,
                None,
                None
            )
            .unwrap(),
            ControlAction::TypeTextSubmit {
                text: "发送".into(),
                x: Some(120),
                y: Some(240)
            }
        );
        assert!(
            ControlAction::parse_ext("type_text_submit", None, None, None, None, None, None)
                .is_err(),
            "缺少 text 时必须结构化报错"
        );
    }

    #[test]
    fn control_action_parse_round90_point_primitives() {
        // 第 90 轮:move_point / middle_click_point / drag_point + modifiers。
        assert_eq!(
            ControlAction::parse_ext("move_point", None, Some(10), Some(20), None, None, None)
                .unwrap(),
            ControlAction::MovePoint { x: 10, y: 20 }
        );
        assert_eq!(
            ControlAction::parse_ext("hover", None, Some(5), Some(6), None, None, None).unwrap(),
            ControlAction::MovePoint { x: 5, y: 6 }
        );
        assert_eq!(
            ControlAction::parse_ext(
                "middle_click_point",
                None,
                Some(7),
                Some(8),
                None,
                None,
                None
            )
            .unwrap(),
            ControlAction::MiddleClickPoint { x: 7, y: 8 }
        );
        // 拖拽:x/y 起点 + x2/y2 终点 + 修饰键
        assert_eq!(
            ControlAction::parse_ext(
                "drag_point",
                None,
                Some(1),
                Some(2),
                Some(3),
                Some(4),
                Some("ctrl".into())
            )
            .unwrap(),
            ControlAction::DragPoint {
                x: 1,
                y: 2,
                x2: 3,
                y2: 4,
                modifiers: Some("ctrl".into())
            }
        );
        // 拖拽缺终点 → 结构化报错
        assert!(
            ControlAction::parse_ext("drag", None, Some(1), Some(2), None, None, None).is_err()
        );
        // 修饰键点击
        assert_eq!(
            ControlAction::parse_ext(
                "click_point",
                None,
                Some(9),
                Some(10),
                None,
                None,
                Some("ctrl+shift".into())
            )
            .unwrap(),
            ControlAction::ClickPoint {
                x: 9,
                y: 10,
                modifiers: Some("ctrl+shift".into())
            }
        );
        // 非法修饰键 → 报错
        assert!(
            ControlAction::parse_ext(
                "click_point",
                None,
                Some(1),
                Some(2),
                None,
                None,
                Some("banana".into())
            )
            .is_err()
        );
        // 新变体 is_point_action 覆盖
        assert!(ControlAction::MovePoint { x: 0, y: 0 }.is_point_action());
        assert!(ControlAction::MiddleClickPoint { x: 0, y: 0 }.is_point_action());
        assert!(ControlAction::DragPoint {
            x: 0,
            y: 0,
            x2: 1,
            y2: 1,
            modifiers: None
        }
        .is_point_action());
    }
}
