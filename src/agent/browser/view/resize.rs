//! 视口自适应:人工拖动有头查看器窗口 → 宿主按新尺寸重排重渲染。
//!
//! 协议(查看器 → 宿主,stdout 按行 JSON):
//! `{"cmd":"resize","w":1920,"h":1080}`
//!
//! * `w`/`h` = **内容区**尺寸(已扣掉工具栏),即宿主侧的新视口尺寸;
//! * 宽高必须都是正整数(`1..=7680` / `1..=4320`),非法值整行忽略(不 panic);
//! * `w`/`h` 缺一不可(允许 `width`/`height` 别名)。
//!
//! 闭环:宿主收到 resize → 更新前台 headed 页的视口 → 重新布局 → 重新渲染
//! → 经帧协议推新帧(PNG 尺寸 = 新视口尺寸)→ 查看器把新帧 **1:1** 画进
//! 内容区(不拉伸),于是「铺满且不变形」。用户连续拖动时以最后一次收到的
//! 尺寸为准(查看器 100ms 一拍,不做节流)。
//!
//! 帧协议方向不变(宿主 → 查看器):`[u32 len][u8 kind][payload]`,
//! kind `'i'` = PNG 帧。

use serde_json::Value;

use super::super::{document, images, layout, style, LsmBrowserManager, TabState};

/// 查看器上报尺寸的合法范围(下界 1:极端窄窗口也允许;上界与宿主视口同量级)。
pub const MIN_VIEW_W: u32 = 1;
pub const MAX_VIEW_W: u32 = 7680;
pub const MIN_VIEW_H: u32 = 1;
pub const MAX_VIEW_H: u32 = 4320;

/// 宿主侧视口 clamp 范围(与 `LsmBrowserManager::set_viewport` 同口径)。
pub const HOST_MIN_W: u32 = 320;
pub const HOST_MAX_W: u32 = 7680;
pub const HOST_MIN_H: u32 = 240;
pub const HOST_MAX_H: u32 = 4320;

/// 从命令 JSON 里取宽高(接受 `w`/`width` 与 `h`/`height` 别名;缺一不可)。
///
/// 负数/小数/字符串/溢出 → `None`(当作没给,整行忽略)。
pub fn size_from_json(v: &Value) -> Option<(u32, u32)> {
    let w = pick_u32(v, &["w", "width"])?;
    let h = pick_u32(v, &["h", "height"])?;
    Some((w, h))
}

fn pick_u32(v: &Value, keys: &[&str]) -> Option<u32> {
    for key in keys {
        if let Some(n) = v.get(*key).and_then(|x| x.as_u64()) {
            return u32::try_from(n).ok();
        }
    }
    None
}

/// 校验查看器上报的尺寸是否合法(1..=7680 × 1..=4320)。
pub fn valid_view_size(w: u32, h: u32) -> bool {
    (MIN_VIEW_W..=MAX_VIEW_W).contains(&w) && (MIN_VIEW_H..=MAX_VIEW_H).contains(&h)
}

/// 宿主侧 clamp(查看器报的尺寸可能超出宿主视口允许范围)。
pub fn clamp_host_size(w: u32, h: u32) -> (u32, u32) {
    (
        w.clamp(HOST_MIN_W, HOST_MAX_W),
        h.clamp(HOST_MIN_H, HOST_MAX_H),
    )
}

/// 生成 resize 命令 JSON(查看器 → 宿主)。
///
/// 与 `macos::cmd_json` 同一风格(手工转义、不引 serde 到 FFI 层);本命令
/// 只有数字,无特殊字符。
pub fn resize_cmd_json(w: u32, h: u32) -> String {
    format!("{{\"cmd\":\"resize\",\"w\":{w},\"h\":{h}}}")
}

impl LsmBrowserManager {
    /// 按 tab 当前视口重新布局(抽出自 `set_viewport`,视口自适应复用同一路径)。
    pub(in crate::agent::browser) fn relayout_tab(&self, tab: &mut TabState) {
        let doc = document::Document::parse(&tab.html, &tab.url);
        let media = style::MediaCtx::new(tab.viewport.width as f64, tab.viewport.height as f64);
        let sizes = images::ImageSizes::from_store(&tab.images);
        tab.layout = layout::compute_full(
            &doc,
            tab.viewport.width as f64,
            &self.font,
            &media,
            &sizes,
        );
    }

    /// 查看器上报了新的内容区尺寸 → 更新视口 → 重排 → 重渲染 → 推新帧。
    ///
    /// * 非 headed 页(没有查看器)忽略;
    /// * 尺寸按宿主视口范围 clamp;
    /// * 宿主重渲染是异步的,期间查看器可能又报新尺寸 → 允许连续覆盖
    ///   (以最后一次收到的尺寸为准)。
    pub(in crate::agent::browser) async fn apply_view_resize(
        &self,
        page_id: &str,
        width: u32,
        height: u32,
    ) {
        let (w, h) = clamp_host_size(width, height);
        let mut inner = self.inner.lock().await;
        let Some(tab) = inner.tabs.get_mut(page_id) else {
            return;
        };
        if !tab.mode.is_headed() {
            return;
        }
        if tab.viewport.width == w && tab.viewport.height == h {
            return;
        }
        tab.viewport.width = w;
        tab.viewport.height = h;
        self.relayout_tab(tab);
        self.push_frame(tab);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::browser::view::{parse_command, ViewCommand};

    fn v(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn 尺寸取值_接受短名与长名别名() {
        assert_eq!(size_from_json(&v(r#"{"w":1920,"h":1080}"#)), Some((1920, 1080)));
        assert_eq!(
            size_from_json(&v(r#"{"width":800,"height":600}"#)),
            Some((800, 600))
        );
        // 混用:短名 + 长名。
        assert_eq!(size_from_json(&v(r#"{"w":800,"height":600}"#)), Some((800, 600)));
        assert_eq!(size_from_json(&v(r#"{"width":800,"h":600}"#)), Some((800, 600)));
    }

    #[test]
    fn 尺寸取值_缺字段或非法类型返回空() {
        // 缺一不可。
        assert_eq!(size_from_json(&v(r#"{"w":1920}"#)), None);
        assert_eq!(size_from_json(&v(r#"{"h":1080}"#)), None);
        assert_eq!(size_from_json(&v(r#"{}"#)), None);
        // 类型不对:字符串/小数/负数/布尔。
        assert_eq!(size_from_json(&v(r#"{"w":"1920","h":"1080"}"#)), None);
        assert_eq!(size_from_json(&v(r#"{"w":1920.5,"h":1080}"#)), None);
        assert_eq!(size_from_json(&v(r#"{"w":-1,"h":1080}"#)), None);
        assert_eq!(size_from_json(&v(r#"{"w":true,"h":1080}"#)), None);
        // 溢出 u32。
        assert_eq!(
            size_from_json(&v(r#"{"w":18446744073709551615,"h":1080}"#)),
            None
        );
    }

    #[test]
    fn 尺寸校验_越界视为非法() {
        assert!(valid_view_size(1920, 1080));
        assert!(valid_view_size(1, 1));
        assert!(!valid_view_size(0, 1080));
        assert!(!valid_view_size(1920, 0));
        assert!(!valid_view_size(MAX_VIEW_W + 1, 1080));
        assert!(!valid_view_size(1920, MAX_VIEW_H + 1));
    }

    #[test]
    fn 宿主侧尺寸_按视口范围夹紧() {
        assert_eq!(clamp_host_size(1920, 1080), (1920, 1080));
        assert_eq!(clamp_host_size(10, 10), (HOST_MIN_W, HOST_MIN_H));
        assert_eq!(clamp_host_size(99999, 99999), (HOST_MAX_W, HOST_MAX_H));
    }

    #[test]
    fn resize命令_格式固定且可被宿主解析() {
        let line = resize_cmd_json(1920, 1080);
        assert_eq!(line, r#"{"cmd":"resize","w":1920,"h":1080}"#);
        let parsed = parse_command(&line);
        assert_eq!(
            parsed,
            Some(ViewCommand::Resize {
                width: 1920,
                height: 1080
            })
        );
    }
}
