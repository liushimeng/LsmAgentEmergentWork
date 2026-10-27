//! 计算样式与 CSS 值解析:长度/颜色/关键字/自定义属性替换。
//!
//! `ComputedStyle` 是布局、渲染、JS 快照、inspect 共用的纯数据真源。
//! 百分比长度按视口宽估算(简化,见设计文档"已知边界")。

use std::collections::BTreeMap;
use std::sync::Arc;

use super::gradient::{parse_gradient, GradientSpec};
use super::transform::{parse_transform, parse_transform_origin, Transform2D, TransformOrigin};

pub type Rgb = [u8; 3];

/// `text-shadow` 单层(阶段六第二步)。
#[derive(Debug, Clone, PartialEq)]
pub struct TextShadow {
    pub dx: f64,
    pub dy: f64,
    pub blur: f64,
    /// `None` = currentColor(布局期代入文字色)。
    pub color: Option<([u8; 3], u8)>,
}

/// `background-image` 单背景层(阶段六第二步,按声明序自顶向下)。
#[derive(Debug, Clone, PartialEq)]
pub enum BgLayer {
    Image(String),
    Gradient(Arc<GradientSpec>),
}

/// 单角圆角半径:长度或百分比(百分比按 `min(w,h)/2` 求解,光栅化期归一)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CornerRadius {
    Px(f64),
    Pct(f64),
}

impl Default for CornerRadius {
    fn default() -> Self {
        CornerRadius::Px(0.0)
    }
}

#[derive(Debug, Clone)]
pub struct ComputedStyle {
    pub display: String,
    pub visibility: String,
    /// `box-sizing`:默认按 border-box 处理(主流站点 `*{box-sizing:border-box}`
    /// 全局重置的既成事实语义,布局层据此从指定宽高中扣减 padding);
    /// 站点显式声明 `content-box` 时置 false,宽高按内容盒语义还原。
    pub box_sizing_content: bool,
    pub color: Rgb,
    pub background: Option<Rgb>,
    pub font_size: f64,
    pub font_weight: u16,
    pub font_family: String,
    pub line_height: f64,
    pub text_align: String,
    pub white_space: String,
    /// CSS `float`(非继承):none/left/right。布局层在块容器内按浮动带布置,
    /// 缺此字段时浮动声明只落在 `extra`、布局完全不可见(浮动卡片纵向堆叠)。
    pub float: String,
    /// CSS `clear`(非继承):none/left/right/both。布置该盒前把游标推到相应浮动底边之下。
    pub clear: String,
    pub margin_top: f64,
    pub margin_right: f64,
    pub margin_bottom: f64,
    pub margin_left: f64,
    pub padding_top: f64,
    pub padding_right: f64,
    pub padding_bottom: f64,
    pub padding_left: f64,
    pub width: Option<f64>,
    pub max_width: Option<f64>,
    pub height: Option<f64>,
    /// `width` 的百分比原值(`50%` → 50.0);布局层按父内容宽二次求解。
    ///
    /// 样式层无法得知父盒尺寸,故百分比在此单独留痕,由布局层按真实包含块解析
    /// (见 `layout/mod.rs` 的百分比宽度分支)。`width` 字段此时仍是按视口宽的
    /// 粗估值,布局层以本字段优先。
    pub width_percent: Option<f64>,
    /// `height` 的百分比原值;父高未知时按 `auto` 处理(见布局层)。
    pub height_percent: Option<f64>,
    pub opacity: f64,
    /// 定位体系(阶段三):position/inset/z-index,全部非继承。
    pub pos: PositionStyle,
    /// Flex 容器属性(阶段三),全部非继承。
    pub flex: FlexContainerStyle,
    /// Flex 条目属性(阶段三),全部非继承。
    pub flex_item: FlexItemStyle,
    /// Grid 容器属性(阶段三第二步),全部非继承。
    pub grid: GridContainerStyle,
    /// Grid 条目属性(阶段三第二步),全部非继承。
    pub grid_item: GridItemStyle,
    /// `object-fit`(阶段三第三步):替换元素内容在盒内的填充方式。
    pub object_fit: String,
    /// `background-image` 背景层(阶段六第二步,按声明序自顶向下);`url()` 与
    /// `gradient()` 混排保序,`background` 简写与长写法都可能命中。
    pub background_layers: Vec<BgLayer>,
    /// `background-position`(精灵图定位;None = 默认 0 0)。
    pub background_position: Option<BackgroundPosition>,
    /// `background-size`:"auto"(默认,按固有尺寸) | "cover" | "contain"。
    pub background_size: String,
    /// 边框线宽(top/right/bottom/left)。默认 0;样式 `none` 侧不参与绘制。
    pub border_widths: [f64; 4],
    /// 边框绘制开关(top/right/bottom/left):`border-style:none/hidden` 为 false。
    pub border_drawn: [bool; 4],
    /// 边框颜色(top/right/bottom/left)。`CurrentColor` 在布局期解析为文字色;
    /// `transparent` 归一为 alpha=0 的 `Rgba`。
    pub border_colors: [BorderColor; 4],
    /// `box-shadow` 外阴影层(内阴影/inset 不在此列,解析期丢弃)。
    /// `blur` 自阶段六起真实生效(三趟盒式模糊近似高斯)。
    pub box_shadow: Vec<BoxShadow>,
    /// `text-shadow` 阴影层(阶段六第二步),按声明序绘制(先声明在下)。
    pub text_shadow: Vec<TextShadow>,
    /// CSS 2D `transform`(阶段六第二步);`None` = 无变换。
    pub transform: Option<Transform2D>,
    /// `transform-origin`(阶段六第二步),布局期按盒尺寸求解,默认 `50% 50%`。
    pub transform_origin: TransformOrigin,
    /// 圆角半径(top-left / top-right / bottom-right / bottom-left)。
    pub border_radius: [CornerRadius; 4],
    /// `overflow:hidden|clip|scroll|auto` 归一为内容裁剪开关(非继承)。
    pub overflow_clip: bool,
    /// 未建模属性的计算值文本(如 border-radius)。
    pub extra: BTreeMap<String, String>,
    /// 自定义属性表(继承用,不进入 `to_map`)。
    pub custom: BTreeMap<String, String>,
}

/// 边框颜色计算值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderColor {
    /// CSS 默认:边框颜色取元素文字色(currentColor),布局期代入。
    CurrentColor,
    Rgba([u8; 3], u8),
}

impl Default for BorderColor {
    fn default() -> Self {
        BorderColor::CurrentColor
    }
}

/// `box-shadow` 单层(仅外阴影)。
#[derive(Debug, Clone, PartialEq)]
pub struct BoxShadow {
    pub dx: f64,
    pub dy: f64,
    /// 高斯模糊半径。渲染层近似为 0(实心偏移矩形),保留字段供后续升级。
    pub blur: f64,
    pub spread: f64,
    /// `None` = currentColor(布局期代入文字色)。
    pub color: Option<([u8; 3], u8)>,
}

/// 定位体系计算值。`sticky` 布局层按 `relative` 处理(见阶段三设计文档边界)。
#[derive(Debug, Clone)]
pub struct PositionStyle {
    pub position: String,
    pub top: Option<f64>,
    pub right: Option<f64>,
    pub bottom: Option<f64>,
    pub left: Option<f64>,
    /// `auto` 归一为 0;绘制序稳定排序使用。
    pub z_index: i64,
}

impl Default for PositionStyle {
    fn default() -> Self {
        Self {
            position: "static".to_string(),
            top: None,
            right: None,
            bottom: None,
            left: None,
            z_index: 0,
        }
    }
}

impl PositionStyle {
    /// 是否建立新的绝对定位包含块(static 之外的所有定位)。
    pub fn is_positioned(&self) -> bool {
        self.position != "static"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexDirection {
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

impl FlexDirection {
    pub fn is_row(&self) -> bool {
        matches!(self, FlexDirection::Row | FlexDirection::RowReverse)
    }
    pub fn is_reverse(&self) -> bool {
        matches!(self, FlexDirection::RowReverse | FlexDirection::ColumnReverse)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexWrap {
    Nowrap,
    Wrap,
    WrapReverse,
}

/// Flex 容器计算值。
#[derive(Debug, Clone)]
pub struct FlexContainerStyle {
    pub direction: FlexDirection,
    pub wrap: FlexWrap,
    pub justify: String,
    pub align_items: String,
    pub align_content: String,
    pub row_gap: f64,
    pub col_gap: f64,
}

impl Default for FlexContainerStyle {
    fn default() -> Self {
        Self {
            direction: FlexDirection::Row,
            wrap: FlexWrap::Nowrap,
            justify: "flex-start".to_string(),
            align_items: "stretch".to_string(),
            align_content: "stretch".to_string(),
            row_gap: 0.0,
            col_gap: 0.0,
        }
    }
}

/// Flex 条目计算值。
#[derive(Debug, Clone)]
pub struct FlexItemStyle {
    pub grow: f64,
    pub shrink: f64,
    pub basis: Option<f64>,
    pub align_self: String,
    pub order: i64,
}

impl Default for FlexItemStyle {
    fn default() -> Self {
        Self {
            grow: 0.0,
            shrink: 1.0,
            basis: None,
            align_self: "auto".to_string(),
            order: 0,
        }
    }
}

/// Grid 轨道尺寸(声明期解析,布局期只读)。
#[derive(Debug, Clone, PartialEq)]
pub enum TrackSize {
    Px(f64),
    /// 列按内容宽;行按容器高(高度不定时退化 auto)。
    Percent(f64),
    Fr(f64),
    Auto,
    MinContent,
    MaxContent,
    /// `minmax(min, max)`;分量不再嵌套 minmax。
    MinMax(Box<TrackSize>, Box<TrackSize>),
}

impl TrackSize {
    /// 分量序列化文本(`to_map` 与 `grid-template-*` 回显用)。
    pub fn text(&self) -> String {
        match self {
            TrackSize::Px(v) => format!("{}px", num(*v)),
            TrackSize::Percent(v) => format!("{}%", num(*v)),
            TrackSize::Fr(v) => format!("{}fr", num(*v)),
            TrackSize::Auto => "auto".into(),
            TrackSize::MinContent => "min-content".into(),
            TrackSize::MaxContent => "max-content".into(),
            TrackSize::MinMax(a, b) => format!("minmax({}, {})", a.text(), b.text()),
        }
    }
}

/// 网格线(条目放置)。负线号与命名线不支持(见设计文档边界)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridLine {
    Auto,
    /// 1 基线号。
    Int(i64),
    Span(u32),
}

/// 自动放置方向。`dense` 后缀接受但按 sparse 处理(见设计文档边界)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridFlow {
    Row,
    Column,
}

/// Grid 容器计算值。
#[derive(Debug, Clone)]
pub struct GridContainerStyle {
    pub template_cols: Vec<TrackSize>,
    pub template_rows: Vec<TrackSize>,
    pub auto_flow: GridFlow,
    pub auto_cols: TrackSize,
    pub auto_rows: TrackSize,
    pub justify_items: String,
    pub align_items: String,
    pub justify_content: String,
    pub align_content: String,
    pub row_gap: f64,
    pub col_gap: f64,
}

impl Default for GridContainerStyle {
    fn default() -> Self {
        Self {
            template_cols: Vec::new(),
            template_rows: Vec::new(),
            auto_flow: GridFlow::Row,
            auto_cols: TrackSize::Auto,
            auto_rows: TrackSize::Auto,
            justify_items: "stretch".to_string(),
            align_items: "stretch".to_string(),
            justify_content: "stretch".to_string(),
            align_content: "stretch".to_string(),
            row_gap: 0.0,
            col_gap: 0.0,
        }
    }
}

/// Grid 条目计算值。
#[derive(Debug, Clone)]
pub struct GridItemStyle {
    pub col_start: GridLine,
    pub col_end: GridLine,
    pub row_start: GridLine,
    pub row_end: GridLine,
    pub justify_self: String,
    pub align_self: String,
}

impl Default for GridItemStyle {
    fn default() -> Self {
        Self {
            col_start: GridLine::Auto,
            col_end: GridLine::Auto,
            row_start: GridLine::Auto,
            row_end: GridLine::Auto,
            justify_self: "auto".to_string(),
            align_self: "auto".to_string(),
        }
    }
}

impl Default for ComputedStyle {
    fn default() -> Self {
        Self {
            display: "inline".to_string(),
            visibility: "visible".to_string(),
            box_sizing_content: false,
            color: [32, 32, 32],
            background: None,
            font_size: 16.0,
            font_weight: 400,
            font_family: String::new(),
            line_height: 16.0 * 1.45,
            text_align: "left".to_string(),
            white_space: "normal".to_string(),
            float: "none".to_string(),
            clear: "none".to_string(),
            margin_top: 0.0,
            margin_right: 0.0,
            margin_bottom: 0.0,
            margin_left: 0.0,
            padding_top: 0.0,
            padding_right: 0.0,
            padding_bottom: 0.0,
            padding_left: 0.0,
            width: None,
            max_width: None,
            height: None,
            width_percent: None,
            height_percent: None,
            opacity: 1.0,
            pos: PositionStyle::default(),
            flex: FlexContainerStyle::default(),
            flex_item: FlexItemStyle::default(),
            grid: GridContainerStyle::default(),
            grid_item: GridItemStyle::default(),
            object_fit: "fill".to_string(),
            background_layers: Vec::new(),
            background_position: None,
            background_size: "auto".to_string(),
            border_widths: [0.0; 4],
            border_drawn: [false; 4],
            border_colors: [BorderColor::CurrentColor; 4],
            box_shadow: Vec::new(),
            text_shadow: Vec::new(),
            transform: None,
            transform_origin: TransformOrigin::default(),
            border_radius: [CornerRadius::Px(0.0); 4],
            overflow_clip: false,
            extra: BTreeMap::new(),
            custom: BTreeMap::new(),
        }
    }
}

impl ComputedStyle {
    /// 文档根元素的计算样式基线。
    pub fn root() -> Self {
        Self {
            display: "block".to_string(),
            box_sizing_content: false,
            ..Default::default()
        }
    }

    pub fn is_block(&self) -> bool {
        matches!(
            self.display.as_str(),
            "block" | "flex" | "grid" | "table" | "table-row" | "list-item"
        )
    }

    pub fn is_hidden(&self) -> bool {
        self.display == "none"
    }

    /// `border-radius` 计算值文本(getComputedStyle 用)。
    fn border_radius_text(&self) -> String {
        let corner = |c: &CornerRadius| match c {
            CornerRadius::Px(v) => format!("{}px", num(*v)),
            CornerRadius::Pct(v) => format!("{}%", num(*v)),
        };
        self.border_radius
            .iter()
            .map(corner)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// 供 `getComputedStyle` / inspect 使用的计算值表(kebab-case)。
    pub fn to_map(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        out.insert("display".into(), self.display.clone());
        out.insert("float".into(), self.float.clone());
        out.insert("clear".into(), self.clear.clone());
        out.insert("visibility".into(), self.visibility.clone());
        out.insert("color".into(), rgb_text(self.color));
        out.insert(
            "background-color".into(),
            self.background
                .map(rgb_text)
                .unwrap_or_else(|| "rgba(0, 0, 0, 0)".into()),
        );
        out.insert("font-size".into(), format!("{}px", num(self.font_size)));
        out.insert("font-weight".into(), self.font_weight.to_string());
        out.insert(
            "font-family".into(),
            if self.font_family.is_empty() {
                "system-ui".to_string()
            } else {
                self.font_family.clone()
            },
        );
        out.insert("line-height".into(), format!("{}px", num(self.line_height)));
        out.insert("text-align".into(), self.text_align.clone());
        out.insert("white-space".into(), self.white_space.clone());
        for (name, v) in [
            ("margin-top", self.margin_top),
            ("margin-right", self.margin_right),
            ("margin-bottom", self.margin_bottom),
            ("margin-left", self.margin_left),
            ("padding-top", self.padding_top),
            ("padding-right", self.padding_right),
            ("padding-bottom", self.padding_bottom),
            ("padding-left", self.padding_left),
        ] {
            out.insert(name.to_string(), format!("{}px", num(v)));
        }
        out.insert("width".into(), length_text(self.width));
        out.insert("max-width".into(), length_text(self.max_width));
        out.insert("height".into(), length_text(self.height));
        out.insert("opacity".into(), num(self.opacity));
        // 定位体系(阶段三)
        out.insert("position".into(), self.pos.position.clone());
        for (name, v) in [
            ("top", self.pos.top),
            ("right", self.pos.right),
            ("bottom", self.pos.bottom),
            ("left", self.pos.left),
        ] {
            out.insert(name.to_string(), length_text(v));
        }
        out.insert("z-index".into(), self.pos.z_index.to_string());
        // Flex(阶段三)
        out.insert(
            "flex-direction".into(),
            match self.flex.direction {
                FlexDirection::Row => "row".to_string(),
                FlexDirection::RowReverse => "row-reverse".to_string(),
                FlexDirection::Column => "column".to_string(),
                FlexDirection::ColumnReverse => "column-reverse".to_string(),
            },
        );
        out.insert(
            "flex-wrap".into(),
            match self.flex.wrap {
                FlexWrap::Nowrap => "nowrap".to_string(),
                FlexWrap::Wrap => "wrap".to_string(),
                FlexWrap::WrapReverse => "wrap-reverse".to_string(),
            },
        );
        out.insert("justify-content".into(), self.flex.justify.clone());
        out.insert("align-items".into(), self.flex.align_items.clone());
        out.insert("align-content".into(), self.flex.align_content.clone());
        out.insert(
            "box-sizing".into(),
            if self.box_sizing_content { "content-box" } else { "border-box" }.to_string(),
        );
        out.insert("row-gap".into(), format!("{}px", num(self.flex.row_gap)));
        out.insert("column-gap".into(), format!("{}px", num(self.flex.col_gap)));
        out.insert("flex-grow".into(), num(self.flex_item.grow));
        out.insert("flex-shrink".into(), num(self.flex_item.shrink));
        out.insert("flex-basis".into(), length_text(self.flex_item.basis));
        out.insert("align-self".into(), self.flex_item.align_self.clone());
        out.insert("order".into(), self.flex_item.order.to_string());
        // Grid(阶段三第二步)
        out.insert(
            "grid-template-columns".into(),
            track_list_text(&self.grid.template_cols),
        );
        out.insert(
            "grid-template-rows".into(),
            track_list_text(&self.grid.template_rows),
        );
        out.insert(
            "grid-auto-flow".into(),
            match self.grid.auto_flow {
                GridFlow::Row => "row".to_string(),
                GridFlow::Column => "column".to_string(),
            },
        );
        out.insert("grid-auto-columns".into(), self.grid.auto_cols.text());
        out.insert("grid-auto-rows".into(), self.grid.auto_rows.text());
        out.insert("justify-items".into(), self.grid.justify_items.clone());
        out.insert(
            "justify-self".into(),
            self.grid_item.justify_self.clone(),
        );
        out.insert("object-fit".into(), self.object_fit.clone());
        out.insert(
            "background-image".into(),
            if self.background_layers.is_empty() {
                "none".to_string()
            } else {
                self.background_layers
                    .iter()
                    .map(|l| match l {
                        BgLayer::Image(u) => format!("url({u})"),
                        BgLayer::Gradient(g) => g.raw.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        );
        // `transform` / `transform-origin` 的解析值含盒尺寸相关量(百分比),
        // 无法在样式层解析为标量,故不进 `to_map`(观测走 DOM 的 inline style 原文)。
        out.insert("background-size".into(), self.background_size.clone());
        out.insert("border-radius".into(), self.border_radius_text());
        out.insert(
            "overflow".into(),
            if self.overflow_clip { "hidden" } else { "visible" }.to_string(),
        );
        out.insert(
            "background-position".into(),
            self.background_position
                .map(|p| p.text())
                .unwrap_or_else(|| "0% 0%".to_string()),
        );
        for (k, v) in &self.extra {
            out.entry(k.clone()).or_insert_with(|| v.clone());
        }
        out
    }
}

fn num(v: f64) -> String {
    if (v - v.round()).abs() < 0.01 {
        format!("{}", v.round() as i64)
    } else {
        format!("{:.2}", v)
    }
}

fn length_text(v: Option<f64>) -> String {
    v.map(|v| format!("{}px", num(v)))
        .unwrap_or_else(|| "auto".into())
}

/// 轨道列表序列化(空表回显 `none`)。
fn track_list_text(tracks: &[TrackSize]) -> String {
    if tracks.is_empty() {
        return "none".into();
    }
    tracks
        .iter()
        .map(|t| t.text())
        .collect::<Vec<_>>()
        .join(" ")
}

fn rgb_text(c: Rgb) -> String {
    format!("rgb({}, {}, {})", c[0], c[1], c[2])
}

/// 上下文:百分比/`rem`/视口相关解析。
#[derive(Debug, Clone, Copy)]
pub struct ValueCtx {
    pub viewport_width: f64,
    pub viewport_height: f64,
    pub root_font_size: f64,
}

impl Default for ValueCtx {
    fn default() -> Self {
        Self {
            viewport_width: 1280.0,
            viewport_height: 720.0,
            root_font_size: 16.0,
        }
    }
}

const BLOCK_TAGS: &[&str] = &[
    "html",
    "body",
    "div",
    "p",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "ul",
    "ol",
    "li",
    "dl",
    "dt",
    "dd",
    "form",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "section",
    "article",
    "header",
    "footer",
    "main",
    "nav",
    "aside",
    "blockquote",
    "pre",
    "figure",
    "figcaption",
    "fieldset",
    "address",
    "hr",
    "details",
    "summary",
    "dialog",
    "center",
];

const HIDDEN_TAGS: &[&str] = &[
    "head", "title", "meta", "link", "style", "script", "noscript", "template", "base",
];

/// 标签的 UA 默认样式。可继承属性从父级复制,其余属性重置。
pub fn ua_default(tag: &str, parent: &ComputedStyle) -> ComputedStyle {
    let tag = tag.to_ascii_lowercase();
    let mut st = ComputedStyle {
        display: if HIDDEN_TAGS.contains(&tag.as_str()) {
            "none".to_string()
        } else if BLOCK_TAGS.contains(&tag.as_str()) {
            "block".to_string()
        } else {
            "inline".to_string()
        },
        // 继承属性
        visibility: parent.visibility.clone(),
        color: parent.color,
        font_size: parent.font_size,
        font_weight: parent.font_weight,
        font_family: parent.font_family.clone(),
        line_height: parent.line_height,
        text_align: parent.text_align.clone(),
        white_space: parent.white_space.clone(),
        ..Default::default()
    };
    match tag.as_str() {
        "h1" => {
            st.font_size = parent.font_size * 2.0;
            st.font_weight = 700;
            st.line_height = st.font_size * 1.2;
        }
        "h2" => {
            st.font_size = parent.font_size * 1.5;
            st.font_weight = 700;
            st.line_height = st.font_size * 1.2;
        }
        "h3" => {
            st.font_size = parent.font_size * 1.17;
            st.font_weight = 700;
            st.line_height = st.font_size * 1.25;
        }
        "h4" => {
            st.font_size = parent.font_size;
            st.font_weight = 700;
            st.line_height = st.font_size * 1.3;
        }
        "h5" => {
            st.font_size = parent.font_size * 0.83;
            st.font_weight = 700;
            st.line_height = st.font_size * 1.35;
        }
        "h6" => {
            st.font_size = parent.font_size * 0.67;
            st.font_weight = 700;
        }
        "strong" | "b" | "th" => st.font_weight = 700,
        "small" => st.font_size = (parent.font_size * 0.85).max(8.0),
        "code" | "kbd" | "samp" => {
            st.font_family = "monospace".to_string();
        }
        "pre" => {
            st.font_family = "monospace".to_string();
            st.white_space = "pre".to_string();
        }
        "center" | "caption" => st.text_align = "center".to_string(),
        // 表单控件 UA 默认描边:真实浏览器给 input/textarea/select/button
        // 默认 1px 灰边框 + 白底;缺它时裸表单控件在渲染输出中完全不可见
        // (页面未自定义控件样式时截图/视觉核验拿不到输入框几何)。
        "input" | "textarea" | "select" | "button" => {
            st.border_widths = [1.0; 4];
            st.border_drawn = [true; 4];
            st.border_colors = [BorderColor::Rgba([118, 118, 118], 255); 4];
            st.background = Some([255, 255, 255]);
        }
        _ => {}
    }
    let margin = match tag.as_str() {
        "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "pre" | "blockquote"
        | "dl" | "figure" => 8.0,
        _ => 0.0,
    };
    if margin > 0.0 {
        st.margin_top = margin;
        st.margin_bottom = margin;
    }
    if tag == "li" {
        st.margin_left = 20.0;
    }
    if tag == "body" {
        st.margin_top = 8.0;
        st.margin_bottom = 8.0;
        st.margin_left = 8.0;
        st.margin_right = 8.0;
    }
    st
}

/// 应用单条声明。`parent` 用于 `inherit`, `root` 用于 `initial`/`revert`。
pub fn apply_declaration(
    st: &mut ComputedStyle,
    key: &str,
    value: &str,
    parent: &ComputedStyle,
    root: &ComputedStyle,
    ctx: &ValueCtx,
) {
    let key = key.trim().to_ascii_lowercase();
    let value = value.trim();
    if key.is_empty() || value.is_empty() {
        return;
    }
    if key.starts_with("--") {
        st.custom.insert(key, value.to_string());
        return;
    }
    if value.eq_ignore_ascii_case("inherit") {
        apply_inherited(st, &key, parent);
        return;
    }
    if value.eq_ignore_ascii_case("initial") || value.eq_ignore_ascii_case("revert") {
        apply_initial(st, &key, root);
        return;
    }
    if value.eq_ignore_ascii_case("unset") {
        if is_inherited_property(&key) {
            apply_inherited(st, &key, parent);
        } else {
            apply_initial(st, &key, root);
        }
        return;
    }
    match key.as_str() {
        "display" => st.display = normalize_display(value),
        "box-sizing" => {
            st.box_sizing_content = value.trim().eq_ignore_ascii_case("content-box");
        }
        "visibility" => {
            st.visibility = value.to_ascii_lowercase();
        }
        "color" => {
            if let Some(c) = resolve_color(value, st.color) {
                st.color = c;
            }
        }
        "background-color" => {
            if value.eq_ignore_ascii_case("transparent") {
                st.background = None;
            } else if let Some(c) = resolve_color(value, st.color) {
                st.background = Some(c);
            }
        }
        "background" => {
            // `background` 是简写:可同时含颜色、url(...)、位置、尺寸
            // (`background:#fff url(a.png) no-repeat -15px -136px`)。
            // 此前只取颜色、**丢掉 url(...) 与位置**,而绝大多数 Web UI 的
            // 图标/按钮/精灵图都用简写写背景图 —— 结果整页 UI 渲染成空白
            // (实测百度地图:搜索框、路线按钮全是背景图精灵,截图一片白)。
            // 多背景层:按顶层逗号切层,逐层收集 url()/gradient();位置/尺寸/颜色
            // 按整值收集(简写的位置/尺寸常只作用于单层,此处取首见,近似)。
            let mut layers: Vec<BgLayer> = Vec::new();
            let mut pos_tokens: Vec<String> = Vec::new();
            let mut size: Option<String> = None;
            let mut color_token: Option<String> = None;
            for seg in split_top_level(value) {
                for t in split_bg_tokens(&seg) {
                    let low = t.to_ascii_lowercase();
                    if low.starts_with("url(") {
                        if let Some(u) = parse_css_url(&t) {
                            layers.push(BgLayer::Image(u));
                        }
                        continue;
                    }
                    if let Some(g) = parse_gradient(&t) {
                        layers.push(BgLayer::Gradient(Arc::new(g)));
                        continue;
                    }
                    if low == "cover" || low == "contain" {
                        size = Some(low);
                        continue;
                    }
                    if low == "auto" {
                        size = Some("auto".to_string());
                        continue;
                    }
                    if is_background_position_token(&low) {
                        pos_tokens.push(t.clone());
                        continue;
                    }
                    if BG_IGNORED_TOKENS.contains(&low.as_str()) {
                        continue;
                    }
                    if parse_color(&t).is_some() {
                        color_token = Some(t.clone());
                    }
                }
            }
            // 简写会重置 background-image / position / size(规范语义)。
            st.background_layers = layers;
            st.background_size = size.unwrap_or_else(|| "auto".to_string());
            st.background_position = if pos_tokens.is_empty() {
                None
            } else {
                parse_background_position(&pos_tokens.join(" "))
            };
            st.background = match color_token {
                Some(c) => parse_color(&c).and_then(|v| v).map(|(rgb, alpha)| blend_on_white(rgb, alpha)),
                None => None,
            };
        }
        "background-position" => {
            st.background_position = parse_background_position(value);
        }
        "background-size" => {
            st.background_size = parse_background_size(value).unwrap_or_else(|| "auto".to_string());
        }
        "font-size" => {
            if let Some(v) = parse_font_size(value, parent.font_size, ctx) {
                st.font_size = v.clamp(6.0, 200.0);
                st.line_height = st.font_size * 1.45;
            }
        }
        "font-weight" => {
            if let Some(v) = parse_font_weight(value, st.font_weight) {
                st.font_weight = v;
            }
        }
        "font-family" => st.font_family = value.trim_matches(|c| c == '"' || c == '\'').to_string(),
        "line-height" => {
            if let Some(v) = parse_line_height(value, st.font_size, ctx) {
                st.line_height = v.clamp(1.0, 500.0);
            }
        }
        "text-align" => {
            let v = value.to_ascii_lowercase();
            let v = match v.as_str() {
                "start" => "left".to_string(),
                "end" => "right".to_string(),
                other => other.to_string(),
            };
            if matches!(v.as_str(), "left" | "right" | "center" | "justify") {
                st.text_align = v;
            }
        }
        "white-space" => {
            let v = value.to_ascii_lowercase();
            if matches!(
                v.as_str(),
                "normal" | "nowrap" | "pre" | "pre-wrap" | "pre-line" | "break-spaces"
            ) {
                st.white_space = v;
            }
        }
        "margin" => {
            let values = parse_box_values(value, st.font_size, ctx);
            if let Some((t, r, b, l)) = expand_box(&values) {
                st.margin_top = t;
                st.margin_right = r;
                st.margin_bottom = b;
                st.margin_left = l;
            }
        }
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" => {
            if let Some(v) = parse_length(value, st.font_size, ctx) {
                match key.as_str() {
                    "margin-top" => st.margin_top = v,
                    "margin-right" => st.margin_right = v,
                    "margin-bottom" => st.margin_bottom = v,
                    _ => st.margin_left = v,
                }
            }
        }
        "padding" => {
            let values = parse_box_values(value, st.font_size, ctx);
            if let Some((t, r, b, l)) = expand_box(&values) {
                st.padding_top = t;
                st.padding_right = r;
                st.padding_bottom = b;
                st.padding_left = l;
            }
        }
        "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
            if let Some(v) = parse_length(value, st.font_size, ctx) {
                match key.as_str() {
                    "padding-top" => st.padding_top = v.max(0.0),
                    "padding-right" => st.padding_right = v.max(0.0),
                    "padding-bottom" => st.padding_bottom = v.max(0.0),
                    _ => st.padding_left = v.max(0.0),
                }
            }
        }
        "width" => {
            st.width_percent = parse_percent(value);
            st.width = parse_length(value, st.font_size, ctx).filter(|v| *v > 0.0);
        }
        "max-width" => {
            if value.eq_ignore_ascii_case("none") {
                st.max_width = None;
            } else {
                st.max_width = parse_length(value, st.font_size, ctx).filter(|v| *v > 0.0);
            }
        }
        "height" => {
            st.height_percent = parse_percent(value);
            // 百分比高度的粗估值按视口宽算没有意义(视口宽≠高),交给布局层按父高求解;
            // 父高未知时按 `auto` 处理。
            st.height = if st.height_percent.is_some() {
                None
            } else {
                parse_length(value, st.font_size, ctx).filter(|v| *v > 0.0)
            };
        }
        "opacity" => {
            if let Ok(v) = value.parse::<f64>() {
                st.opacity = v.clamp(0.0, 1.0);
            }
        }
        // ============ 定位体系(阶段三) ============
        "position" => {
            let v = value.to_ascii_lowercase();
            if matches!(
                v.as_str(),
                "static" | "relative" | "absolute" | "fixed" | "sticky"
            ) {
                st.pos.position = v;
            }
        }
        "top" | "right" | "bottom" | "left" => {
            let v = parse_length(value, st.font_size, ctx);
            match key.as_str() {
                "top" => st.pos.top = v,
                "right" => st.pos.right = v,
                "bottom" => st.pos.bottom = v,
                _ => st.pos.left = v,
            }
        }
        "z-index" => {
            if value.eq_ignore_ascii_case("auto") {
                st.pos.z_index = 0;
            } else if let Ok(v) = value.parse::<i64>() {
                st.pos.z_index = v;
            }
        }
        // ============ Flex 容器(阶段三) ============
        "flex-direction" => {
            st.flex.direction = match value.to_ascii_lowercase().as_str() {
                "row-reverse" => FlexDirection::RowReverse,
                "column" => FlexDirection::Column,
                "column-reverse" => FlexDirection::ColumnReverse,
                _ => FlexDirection::Row,
            };
        }
        "flex-wrap" => {
            st.flex.wrap = match value.to_ascii_lowercase().as_str() {
                "wrap" => FlexWrap::Wrap,
                "wrap-reverse" => FlexWrap::WrapReverse,
                _ => FlexWrap::Nowrap,
            };
        }
        "justify-content" => {
            if let Some(v) = normalize_flex_align(value) {
                // Flex/Grid 共享关键字:双写,容器只读自己那侧。
                st.flex.justify = v.clone();
                st.grid.justify_content = to_grid_align(&v);
            }
        }
        "align-items" => {
            if let Some(v) = normalize_cross_align(value) {
                st.flex.align_items = v.clone();
                st.grid.align_items = to_grid_align(&v);
            }
        }
        "align-content" => {
            if let Some(v) = normalize_flex_align(value) {
                st.flex.align_content = v.clone();
                st.grid.align_content = to_grid_align(&v);
            } else if value.eq_ignore_ascii_case("stretch") {
                st.flex.align_content = "stretch".to_string();
                st.grid.align_content = "stretch".to_string();
            }
        }
        "gap" | "row-gap" | "column-gap" => {
            let parts: Vec<Option<f64>> = value
                .split_whitespace()
                .map(|p| parse_length(p, st.font_size, ctx))
                .collect();
            match key.as_str() {
                "gap" => {
                    if let Some(first) = parts.first().copied().flatten() {
                        let col = parts
                            .get(1)
                            .copied()
                            .flatten()
                            .unwrap_or(first)
                            .max(0.0);
                        let row = first.max(0.0);
                        // Flex/Grid 共享关键字:双写。
                        st.flex.row_gap = row;
                        st.flex.col_gap = col;
                        st.grid.row_gap = row;
                        st.grid.col_gap = col;
                    }
                }
                "row-gap" => {
                    if let Some(v) = parts.first().copied().flatten() {
                        let v = v.max(0.0);
                        st.flex.row_gap = v;
                        st.grid.row_gap = v;
                    }
                }
                _ => {
                    if let Some(v) = parts.first().copied().flatten() {
                        let v = v.max(0.0);
                        st.flex.col_gap = v;
                        st.grid.col_gap = v;
                    }
                }
            }
        }
        // ============ Flex 条目(阶段三) ============
        "flex-grow" => {
            if let Ok(v) = value.parse::<f64>() {
                st.flex_item.grow = v.max(0.0);
            }
        }
        "flex-shrink" => {
            if let Ok(v) = value.parse::<f64>() {
                st.flex_item.shrink = v.max(0.0);
            }
        }
        "flex-basis" => {
            let v = value.to_ascii_lowercase();
            if matches!(v.as_str(), "auto" | "content" | "fill" | "fit-content" | "max-content") {
                st.flex_item.basis = None;
            } else {
                st.flex_item.basis = parse_length(value, st.font_size, ctx).filter(|v| *v > 0.0);
            }
        }
        "align-self" => {
            let v = value.to_ascii_lowercase();
            if matches!(
                v.as_str(),
                "auto" | "stretch" | "flex-start" | "flex-end" | "center" | "baseline"
            ) {
                // Flex/Grid 共享关键字:双写(start/end 归一到 flex-* 域)。
                st.flex_item.align_self = v.clone();
                st.grid_item.align_self = to_grid_align(&v);
            }
        }
        "order" => {
            if let Ok(v) = value.parse::<i64>() {
                st.flex_item.order = v;
            }
        }
        "flex" => apply_flex_shorthand(st, value, ctx),
        // ============ Grid 容器(阶段三第二步) ============
        "grid-template-columns" => {
            st.grid.template_cols = parse_track_list(value, st.font_size, ctx);
        }
        "grid-template-rows" => {
            st.grid.template_rows = parse_track_list(value, st.font_size, ctx);
        }
        "grid-auto-columns" => {
            if let Some(t) = parse_track_size(value, st.font_size, ctx) {
                st.grid.auto_cols = t;
            }
        }
        "grid-auto-rows" => {
            if let Some(t) = parse_track_size(value, st.font_size, ctx) {
                st.grid.auto_rows = t;
            }
        }
        "grid-auto-flow" => {
            let v = value.to_ascii_lowercase();
            st.grid.auto_flow = if v.contains("column") {
                GridFlow::Column
            } else {
                GridFlow::Row
            };
        }
        "justify-items" => {
            if let Some(v) = normalize_grid_align(value) {
                st.grid.justify_items = v;
            }
        }
        "justify-self" => {
            if let Some(v) = normalize_grid_align(value) {
                st.grid_item.justify_self = v;
            }
        }
        // ============ Grid 条目放置(阶段三第二步) ============
        "grid-column" | "grid-row" => {
            let (start, end) = parse_grid_line_shorthand(value);
            match key.as_str() {
                "grid-column" => {
                    st.grid_item.col_start = start;
                    st.grid_item.col_end = end;
                }
                _ => {
                    st.grid_item.row_start = start;
                    st.grid_item.row_end = end;
                }
            }
        }
        "grid-column-start" => st.grid_item.col_start = parse_grid_line(value),
        "grid-column-end" => st.grid_item.col_end = parse_grid_line(value),
        "grid-row-start" => st.grid_item.row_start = parse_grid_line(value),
        "grid-row-end" => st.grid_item.row_end = parse_grid_line(value),
        "background-image" => {
            st.background_layers = parse_bg_layers(value);
        }
        "object-fit" => {
            let v = value.trim().to_ascii_lowercase();
            if matches!(
                v.as_str(),
                "fill" | "contain" | "cover" | "none" | "scale-down"
            ) {
                st.object_fit = v;
            }
        }
        // ---- 边框(绘制用结构化解析;top/right/bottom/left = 0/1/2/3)----
        "border" => {
            let (w, drawn, color) = parse_border_shorthand(value);
            for side in 0..4 {
                if let Some(w) = w {
                    st.border_widths[side] = w;
                }
                if let Some(d) = drawn {
                    st.border_drawn[side] = d;
                }
                if let Some(c) = color {
                    st.border_colors[side] = c;
                }
            }
        }
        "border-width" => {
            let ws = parse_border_width_list(value, st.font_size, ctx);
            apply_quad(&mut st.border_widths, ws);
        }
        "border-color" => {
            let cs = parse_border_color_list(value);
            apply_quad(&mut st.border_colors, cs);
        }
        "border-style" => {
            let ds = parse_border_style_list(value);
            apply_quad(&mut st.border_drawn, ds);
        }
        "border-top" | "border-right" | "border-bottom" | "border-left" => {
            let side = match key.as_str() {
                "border-top" => 0,
                "border-right" => 1,
                "border-bottom" => 2,
                _ => 3,
            };
            let (w, drawn, color) = parse_border_shorthand(value);
            if let Some(w) = w {
                st.border_widths[side] = w;
            }
            if let Some(d) = drawn {
                st.border_drawn[side] = d;
            }
            if let Some(c) = color {
                st.border_colors[side] = c;
            }
        }
        "border-top-width" | "border-right-width" | "border-bottom-width" | "border-left-width" => {
            let side = match key.as_str() {
                "border-top-width" => 0,
                "border-right-width" => 1,
                "border-bottom-width" => 2,
                _ => 3,
            };
            if let Some(w) = parse_length(value.trim(), st.font_size, ctx) {
                st.border_widths[side] = w;
            }
        }
        "border-top-color" | "border-right-color" | "border-bottom-color" | "border-left-color" => {
            let side = match key.as_str() {
                "border-top-color" => 0,
                "border-right-color" => 1,
                "border-bottom-color" => 2,
                _ => 3,
            };
            if let Some(c) = parse_color(value) {
                st.border_colors[side] = match c {
                    Some(rgba) => BorderColor::Rgba(rgba.0, rgba.1),
                    None => BorderColor::Rgba([0, 0, 0], 0),
                };
            }
        }
        "border-top-style" | "border-right-style" | "border-bottom-style" | "border-left-style" => {
            let side = match key.as_str() {
                "border-top-style" => 0,
                "border-right-style" => 1,
                "border-bottom-style" => 2,
                _ => 3,
            };
            let v = value.trim().to_ascii_lowercase();
            st.border_drawn[side] = !matches!(v.as_str(), "none" | "hidden" | "");
        }
        "box-shadow" => {
            st.box_shadow = parse_box_shadow(value);
        }
        "text-shadow" => {
            st.text_shadow = parse_text_shadow(value);
        }
        "transform" => {
            st.transform = parse_transform(value);
        }
        "transform-origin" => {
            st.transform_origin = parse_transform_origin(value, st.font_size, ctx);
        }
        "border-radius" | "-webkit-border-radius" => {
            st.border_radius = parse_border_radius(value, st.font_size, ctx);
        }
        "overflow" => {
            st.overflow_clip = parse_overflow(value);
        }
        "overflow-x" | "overflow-y" => {
            // 任一轴裁剪即整体裁剪(视觉近似,滚动条不建模)。
            if parse_overflow(value) {
                st.overflow_clip = true;
            }
        }
        "float" => {
            st.float = match value.to_ascii_lowercase().as_str() {
                "left" | "inline-start" => "left".to_string(),
                "right" | "inline-end" => "right".to_string(),
                _ => "none".to_string(),
            };
        }
        "clear" => {
            st.clear = match value.to_ascii_lowercase().as_str() {
                "left" | "right" | "both" => value.to_ascii_lowercase(),
                _ => "none".to_string(),
            };
        }
        "text-decoration"
        | "cursor"
        | "letter-spacing"
        | "word-spacing"
        | "list-style"
        | "list-style-type"
        | "transition" => {
            st.extra.insert(key, value.to_string());
        }
        _ => {
            st.extra.insert(key, value.to_string());
        }
    }
}

fn is_inherited_property(key: &str) -> bool {
    matches!(
        key,
        "color"
            | "font-size"
            | "font-weight"
            | "font-family"
            | "line-height"
            | "text-align"
            | "white-space"
            | "visibility"
            | "cursor"
            | "letter-spacing"
            | "word-spacing"
            | "list-style"
            | "list-style-type"
    )
}

fn apply_inherited(st: &mut ComputedStyle, key: &str, parent: &ComputedStyle) {
    match key {
        "color" => st.color = parent.color,
        "font-size" => {
            st.font_size = parent.font_size;
            st.line_height = parent.line_height;
        }
        "font-weight" => st.font_weight = parent.font_weight,
        "font-family" => st.font_family = parent.font_family.clone(),
        "line-height" => st.line_height = parent.line_height,
        "text-align" => st.text_align = parent.text_align.clone(),
        "white-space" => st.white_space = parent.white_space.clone(),
        "visibility" => st.visibility = parent.visibility.clone(),
        "display" => st.display = parent.display.clone(),
        "float" => st.float = parent.float.clone(),
        "clear" => st.clear = parent.clear.clone(),
        other => {
            if let Some(v) = parent.extra.get(other) {
                st.extra.insert(other.to_string(), v.clone());
            }
        }
    }
}

fn apply_initial(st: &mut ComputedStyle, key: &str, root: &ComputedStyle) {
    match key {
        "color" => st.color = [0, 0, 0],
        "font-size" => {
            st.font_size = 16.0;
            st.line_height = 16.0 * 1.45;
        }
        "font-weight" => st.font_weight = 400,
        "font-family" => st.font_family.clear(),
        "line-height" => st.line_height = st.font_size * 1.45,
        "text-align" => st.text_align = "left".to_string(),
        "white-space" => st.white_space = "normal".to_string(),
        "visibility" => st.visibility = "visible".to_string(),
        "display" => st.display = "inline".to_string(),
        "float" => st.float = "none".to_string(),
        "clear" => st.clear = "none".to_string(),
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" => {
            let _ = root;
            match key {
                "margin-top" => st.margin_top = 0.0,
                "margin-right" => st.margin_right = 0.0,
                "margin-bottom" => st.margin_bottom = 0.0,
                _ => st.margin_left = 0.0,
            }
        }
        "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => match key {
            "padding-top" => st.padding_top = 0.0,
            "padding-right" => st.padding_right = 0.0,
            "padding-bottom" => st.padding_bottom = 0.0,
            _ => st.padding_left = 0.0,
        },
        "width" | "height" | "max-width" => match key {
            "width" => st.width = None,
            "height" => st.height = None,
            _ => st.max_width = None,
        },
        "background" | "background-color" => st.background = None,
        "object-fit" => st.object_fit = "fill".to_string(),
        "background-image" => st.background_layers.clear(),
        "background-position" => st.background_position = None,
        "background-size" => st.background_size = "auto".to_string(),
        "opacity" => st.opacity = 1.0,
        "box-shadow" => st.box_shadow.clear(),
        "text-shadow" => st.text_shadow.clear(),
        "transform" => st.transform = None,
        "transform-origin" => st.transform_origin = TransformOrigin::default(),
        "border-radius" | "-webkit-border-radius" => st.border_radius = [CornerRadius::Px(0.0); 4],
        "overflow" | "overflow-x" | "overflow-y" => st.overflow_clip = false,
        _ => {}
    }
}

/// 解析 `url(...)` 形式的资源地址(阶段三第三步:背景图)。
///
/// 只取单个 `url()`;`none`/多背景/渐变返回 `None`。
/// `background-position` 单轴取值:像素偏移或百分比(关键字归一为百分比)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackgroundPos {
    Px(f64),
    Percent(f64),
}

impl BackgroundPos {
    /// 布局期按「盒尺寸 - 图尺寸」解析成像素偏移(CSS 定位公式)。
    pub fn resolve(self, box_size: f64, img_size: f64) -> f64 {
        match self {
            BackgroundPos::Px(p) => p,
            BackgroundPos::Percent(p) => (box_size - img_size) * p / 100.0,
        }
    }

    fn text(self) -> String {
        match self {
            BackgroundPos::Px(p) => format!("{p}px"),
            BackgroundPos::Percent(p) => format!("{p}%"),
        }
    }
}

/// `background-position`(两轴)。`left/top/center/right/bottom` 归一为百分比,
/// 故 `right bottom` 与 `100% 100%` 等价。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BackgroundPosition {
    pub x: BackgroundPos,
    pub y: BackgroundPos,
}

impl BackgroundPosition {
    pub fn text(self) -> String {
        format!("{} {}", self.x.text(), self.y.text())
    }
}

/// 解析 `background-position` 长写/简写里的位置部分。
///
/// 支持:`left|center|right` / `top|center|bottom` / `<n>px` / `<n>%`,以及
/// 1 个值(另一轴取 center)或 2 个值的组合。
pub fn parse_background_position(value: &str) -> Option<BackgroundPosition> {
    let tokens: Vec<&str> = value
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return None;
    }
    let mut parts: Vec<BackgroundPos> = Vec::new();
    let mut vertical_first = false;
    for t in &tokens {
        let low = t.to_ascii_lowercase();
        let pos = match low.as_str() {
            "left" => Some((BackgroundPos::Percent(0.0), true)),
            "right" => Some((BackgroundPos::Percent(100.0), true)),
            "top" => Some((BackgroundPos::Percent(0.0), false)),
            "bottom" => Some((BackgroundPos::Percent(100.0), false)),
            "center" => Some((BackgroundPos::Percent(50.0), true)),
            "middle" => Some((BackgroundPos::Percent(50.0), false)),
            _ => {
                if let Some(p) = low.strip_suffix("px") {
                    p.trim().parse::<f64>().ok().map(|v| (BackgroundPos::Px(v), true))
                } else if let Some(p) = low.strip_suffix('%') {
                    p.trim().parse::<f64>().ok().map(|v| (BackgroundPos::Percent(v), true))
                } else {
                    None
                }
            }
        };
        match pos {
            Some((p, horizontal)) => {
                if !parts.is_empty() && !horizontal {
                    vertical_first = true;
                }
                parts.push(p);
            }
            None => return None,
        }
    }
    match parts.len() {
        1 => {
            // 单值:水平关键字/长度 → 垂直取 center;垂直关键字 → 水平取 center。
            if vertical_first {
                Some(BackgroundPosition {
                    x: BackgroundPos::Percent(50.0),
                    y: parts[0],
                })
            } else {
                Some(BackgroundPosition {
                    x: parts[0],
                    y: BackgroundPos::Percent(50.0),
                })
            }
        }
        2 => {
            if vertical_first {
                Some(BackgroundPosition {
                    x: parts[1],
                    y: parts[0],
                })
            } else {
                Some(BackgroundPosition {
                    x: parts[0],
                    y: parts[1],
                })
            }
        }
        _ => None,
    }
}

/// 解析 `background-size` 长写:`cover` / `contain` / `auto` / `<w> <h>`。
pub fn parse_background_size(value: &str) -> Option<String> {
    let low = value.trim().to_ascii_lowercase();
    match low.as_str() {
        "cover" | "contain" | "auto" => Some(low),
        _ => {
            // 只识别「一维/两维长度或百分比」;其他(如 `100px auto`)按 auto 处理。
            let parts: Vec<&str> = low.split_whitespace().collect();
            if parts.is_empty() || parts.len() > 2 {
                return None;
            }
            for p in &parts {
                let ok = p.ends_with("px")
                    || p.ends_with('%')
                    || *p == "auto"
                    || p.parse::<f64>().is_ok();
                if !ok {
                    return None;
                }
            }
            Some(low)
        }
    }
}

/// `background` 简写拆记号(括号内不拆,`url(...)` 保持完整)。
fn split_bg_tokens(value: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    for ch in value.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                cur.push(ch);
            }
            c if depth == 0 && c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 是否是 `background` 简写里的位置记号(长度/百分比/方位关键字)。
fn is_background_position_token(low: &str) -> bool {
    matches!(
        low,
        "left" | "right" | "top" | "bottom" | "center" | "middle"
    ) || low.ends_with("px")
        || low.ends_with('%')
}

/// `background` 简写里的非颜色、非位置记号(重复/附着/盒子),解析时忽略。
const BG_IGNORED_TOKENS: &[&str] = &[
    "no-repeat", "repeat", "repeat-x", "repeat-y", "space", "round", "scroll", "fixed",
    "local", "border-box", "padding-box", "content-box", "none",
];

pub fn parse_css_url(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let pos = lower.find("url(")?;
    let start = pos + 4;
    let end = lower[start..].find(')')? + start;
    let raw = value[start..end].trim().trim_matches(|c| c == '"' || c == '\'').trim();
    if raw.is_empty() {
        None
    } else {
        Some(raw.to_string())
    }
}

/// `overflow` 关键字归一:hidden/clip/scroll/auto 均裁剪,visible 不裁。
fn parse_overflow(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "hidden" | "clip" | "scroll" | "auto" | "overlay"
    )
}

/// `border-radius` 简写:1-4 值 + 可选 `/` 椭圆第二组(取水平组,垂直组忽略)。
/// 值为长度(px 等)或百分比;按 CSS 简写轮转展开到四角 tl/tr/br/bl。
fn parse_border_radius(value: &str, font_size: f64, ctx: &ValueCtx) -> [CornerRadius; 4] {
    let horiz = value.split('/').next().unwrap_or(value);
    let toks: Vec<&str> = horiz.split_whitespace().collect();
    if toks.is_empty() {
        return [CornerRadius::Px(0.0); 4];
    }
    let parse_one = |t: &str| -> CornerRadius {
        let t = t.trim();
        if let Some(pct) = t.strip_suffix('%') {
            if let Ok(v) = pct.trim().parse::<f64>() {
                return CornerRadius::Pct(v.clamp(0.0, 100.0));
            }
        }
        parse_length(t, font_size, ctx)
            .filter(|v| *v > 0.0)
            .map(CornerRadius::Px)
            .unwrap_or_default()
    };
    let [a, b, c, d] = match toks.len() {
        1 => [toks[0], toks[0], toks[0], toks[0]],
        2 => [toks[0], toks[1], toks[0], toks[1]],
        3 => [toks[0], toks[1], toks[2], toks[1]],
        _ => [toks[0], toks[1], toks[2], toks[3]],
    };
    [parse_one(a), parse_one(b), parse_one(c), parse_one(d)]
}

/// `background-image` 多背景层解析:按**顶层**逗号切层,逐层识别 `url()` 与
/// `gradient()`,保持声明序(自顶向下)。
///
/// 注意不可对整值直接按逗号切——渐变函数参数自带逗号
/// (`linear-gradient(to right, a, b)`);`split_top_level` 做括号感知切分,
/// 每层再逐 token 识别。
fn parse_bg_layers(value: &str) -> Vec<BgLayer> {
    let mut out = Vec::new();
    for seg in split_top_level(value) {
        for t in split_bg_tokens(&seg) {
            let low = t.to_ascii_lowercase();
            if low.starts_with("url(") {
                if let Some(u) = parse_css_url(&t) {
                    out.push(BgLayer::Image(u));
                }
            } else if let Some(g) = parse_gradient(&t) {
                out.push(BgLayer::Gradient(Arc::new(g)));
            }
        }
    }
    out
}

/// `text-shadow` 解析:逐层取 `offset-x offset-y [blur] [color]`,层间按
/// **括号外**逗号切分;颜色缺省为 currentColor(`None`),`transparent` 层丢弃。
pub fn parse_text_shadow(value: &str) -> Vec<TextShadow> {
    let mut out = Vec::new();
    let mut layers: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for ch in value.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth <= 0 => layers.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    layers.push(cur);
    for layer in &layers {
        let toks = split_ws_tokens(layer);
        let mut lens = Vec::new();
        let mut color: Option<([u8; 3], u8)> = None;
        let mut color_seen = false;
        for tok in &toks {
            if !color_seen {
                if let Some(c) = parse_color(tok) {
                    color = c;
                    color_seen = true;
                    continue;
                }
            }
            if let Some(l) = parse_length(tok, 16.0, &ValueCtx::default()) {
                lens.push(l);
            }
        }
        if color_seen && color.is_none() {
            continue; // transparent 阴影不可见
        }
        if lens.len() < 2 {
            continue; // 少于 dx/dy,非法层
        }
        out.push(TextShadow {
            dx: lens[0],
            dy: lens[1],
            blur: lens.get(2).copied().unwrap_or(0.0),
            color,
        });
    }
    out
}

fn normalize_display(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => "none".into(),
        "block" => "block".into(),
        "inline" => "inline".into(),
        "inline-block" => "inline-block".into(),
        "flex" | "inline-flex" => "flex".into(),
        "grid" | "inline-grid" => "grid".into(),
        "table" | "inline-table" => "table".into(),
        "table-row" => "table-row".into(),
        "table-cell" => "table-cell".into(),
        "list-item" => "list-item".into(),
        "contents" => "contents".into(),
        other => other.to_string(),
    }
}

// ===================== 长度 / 字号 / 行高 =====================

/// 解析百分比原值(`50%` → 50.0);非百分比/无法识别返回 `None`。
///
/// 百分比长度的最终值依赖包含块尺寸,样式层无从得知,故单独留痕交布局层求解。
/// 按空白切分 CSS 分量,括号内(`rgba(...)`/`var(...)`)不切开。
fn split_ws_tokens(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for ch in value.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                cur.push(ch);
            }
            c if c.is_whitespace() && depth <= 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `border` 单边/四边速写解析:`<width> || <style> || <color>` 任意顺序。
/// 返回(线宽, 是否绘制, 颜色);未出现的分量为 `None`。
/// 速写给出可绘制样式而未给线宽时按 CSS 默认 `medium`(3px)。
pub fn parse_border_shorthand(value: &str) -> (Option<f64>, Option<bool>, Option<BorderColor>) {
    let mut width: Option<f64> = None;
    let mut style_drawn: Option<bool> = None;
    let mut color: Option<BorderColor> = None;
    for tok in split_ws_tokens(value) {
        let t = tok.trim().to_ascii_lowercase();
        if t.is_empty() {
            continue;
        }
        match t.as_str() {
            "none" | "hidden" => {
                style_drawn = Some(false);
                continue;
            }
            "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset" => {
                style_drawn = Some(true);
                continue;
            }
            _ => {}
        }
        if let Some(c) = parse_color(&t) {
            color = Some(match c {
                Some(rgba) => BorderColor::Rgba(rgba.0, rgba.1),
                None => BorderColor::Rgba([0, 0, 0], 0),
            });
            continue;
        }
        if let Some(w) = parse_length(&t, 16.0, &ValueCtx::default()) {
            width = Some(w);
        }
    }
    if style_drawn == Some(true) && width.is_none() {
        width = Some(3.0);
    }
    (width, style_drawn, color)
}

/// `border-width` 速写:1~4 个长度按 CSS 四值规则铺到 [top,right,bottom,left]。
fn parse_border_width_list(value: &str, font_size: f64, ctx: &ValueCtx) -> Vec<f64> {
    split_ws_tokens(value)
        .iter()
        .filter_map(|t| parse_length(t, font_size, ctx))
        .collect()
}

/// `border-color` 速写:1~4 个颜色按四值规则展开。
fn parse_border_color_list(value: &str) -> Vec<BorderColor> {
    let mut out = Vec::new();
    for tok in split_ws_tokens(value) {
        if let Some(c) = parse_color(&tok) {
            out.push(match c {
                Some(rgba) => BorderColor::Rgba(rgba.0, rgba.1),
                None => BorderColor::Rgba([0, 0, 0], 0),
            });
        }
    }
    out
}

/// `border-style` 速写:可绘制性(非 none/hidden)按四值规则展开。
fn parse_border_style_list(value: &str) -> Vec<bool> {
    split_ws_tokens(value)
        .iter()
        .map(|t| !matches!(t.trim().to_ascii_lowercase().as_str(), "none" | "hidden"))
        .collect()
}

/// CSS 四值铺填:1 值=全边,2 值=上下/左右,3 值=上/左右/下,4 值=上右下左。
fn apply_quad<T: Copy>(slot: &mut [T; 4], values: Vec<T>) {
    match values.len() {
        1 => *slot = [values[0]; 4],
        2 => *slot = [values[0], values[1], values[0], values[1]],
        3 => *slot = [values[0], values[1], values[2], values[1]],
        4 => *slot = [values[0], values[1], values[2], values[3]],
        _ => {}
    }
}

/// `box-shadow` 解析:逐层取外阴影(`inset` 层丢弃),长度分量按
/// dx/dy/[blur]/[spread] 顺序,颜色缺省为 currentColor(`None`)。
/// 颜色经 `parse_color`:transparent 层直接丢弃(不可见)。
/// 层间按**括号外**逗号切分(`rgba(...)` 内的逗号不拆层)。
pub fn parse_box_shadow(value: &str) -> Vec<BoxShadow> {
    let mut out = Vec::new();
    // 括号外逗号切层。
    let mut layers: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for ch in value.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth <= 0 => {
                layers.push(std::mem::take(&mut cur));
            }
            c => cur.push(c),
        }
    }
    layers.push(cur);
    for layer in &layers {
        let toks = split_ws_tokens(layer);
        if toks.iter().any(|t| t.trim().eq_ignore_ascii_case("inset")) {
            continue;
        }
        let mut lens = Vec::new();
        let mut color: Option<([u8; 3], u8)> = None;
        let mut color_seen = false;
        for tok in &toks {
            if !color_seen {
                if let Some(c) = parse_color(tok) {
                    match c {
                        Some(rgba) => color = Some(rgba),
                        None => color = None, // transparent 层
                    }
                    color_seen = true;
                    continue;
                }
            }
            if let Some(l) = parse_length(tok, 16.0, &ValueCtx::default()) {
                lens.push(l);
            }
        }
        if color_seen && color.is_none() {
            continue; // transparent 阴影不可见
        }
        if lens.len() < 2 {
            continue; // 少于 dx/dy,非法层
        }
        out.push(BoxShadow {
            dx: lens[0],
            dy: lens[1],
            blur: lens.get(2).copied().unwrap_or(0.0),
            spread: lens.get(3).copied().unwrap_or(0.0),
            color,
        });
    }
    out
}

pub fn parse_percent(value: &str) -> Option<f64> {
    let v = value.trim();
    v.strip_suffix('%')
        .and_then(|n| n.trim().parse::<f64>().ok())
        .filter(|n| n.is_finite())
}

/// 解析长度。`auto`/无法识别返回 `None`。
pub fn parse_length(value: &str, font_size: f64, ctx: &ValueCtx) -> Option<f64> {
    let v = value.trim().to_ascii_lowercase();
    if v.is_empty() || v == "auto" || v == "none" {
        return None;
    }
    // calc(<表达式>):现代站点布局的大量使用(zhipin 全站 50+ 处),此前
    // 整条声明被静默丢弃(元素尺寸回退 auto,布局大面积塌缩错位)。
    if v.starts_with("calc(") && v.ends_with(')') {
        let inner = &v[5..v.len() - 1];
        // 内嵌 calc(...) 就地展开成普通括号(calc 语义上只是分组)。
        let inner = inner.replace("calc(", "(");
        return eval_calc(&inner, font_size, ctx);
    }
    if let Some(n) = v.strip_suffix("px") {
        return n.trim().parse::<f64>().ok();
    }
    if let Some(n) = v.strip_suffix("rem") {
        return n.trim().parse::<f64>().ok().map(|n| n * ctx.root_font_size);
    }
    if let Some(n) = v.strip_suffix("em") {
        return n.trim().parse::<f64>().ok().map(|n| n * font_size);
    }
    if let Some(n) = v.strip_suffix("vw") {
        return n.trim().parse::<f64>().ok().map(|n| n / 100.0 * ctx.viewport_width);
    }
    if let Some(n) = v.strip_suffix("vh") {
        return n.trim().parse::<f64>().ok().map(|n| n / 100.0 * ctx.viewport_height);
    }
    if let Some(n) = v.strip_suffix("vmin") {
        return n
            .trim()
            .parse::<f64>()
            .ok()
            .map(|n| n / 100.0 * ctx.viewport_width.min(ctx.viewport_height));
    }
    if let Some(n) = v.strip_suffix("vmax") {
        return n
            .trim()
            .parse::<f64>()
            .ok()
            .map(|n| n / 100.0 * ctx.viewport_width.max(ctx.viewport_height));
    }
    if let Some(n) = v.strip_suffix('%') {
        return n
            .trim()
            .parse::<f64>()
            .ok()
            .map(|n| n / 100.0 * ctx.viewport_width);
    }
    if let Some(n) = v.strip_suffix("pt") {
        return n.trim().parse::<f64>().ok().map(|n| n * 4.0 / 3.0);
    }
    if let Some(n) = v.strip_suffix("pc") {
        return n.trim().parse::<f64>().ok().map(|n| n * 16.0);
    }
    if let Some(n) = v.strip_suffix("in") {
        return n.trim().parse::<f64>().ok().map(|n| n * 96.0);
    }
    if let Some(n) = v.strip_suffix("cm") {
        return n.trim().parse::<f64>().ok().map(|n| n * 37.795);
    }
    if let Some(n) = v.strip_suffix("mm") {
        return n.trim().parse::<f64>().ok().map(|n| n * 3.7795);
    }
    v.parse::<f64>().ok()
}

/// calc() 表达式求值(长度子集)。
///
/// 支持加减乘除、括号、一元负号与全部 `parse_length` 单位;`%` 沿用
/// 解析层的视口宽近似(与裸百分比的既有回退语义一致,布局层对 width/height
/// 的精确百分比求解不受影响——那些在声明捕获阶段就走 `width_percent` 通道)。
fn eval_calc(expr: &str, font_size: f64, ctx: &ValueCtx) -> Option<f64> {
    let tokens = tokenize_calc(expr)?;
    let mut pos = 0usize;
    let v = calc_expr(&tokens, &mut pos, font_size, ctx)?;
    if pos != tokens.len() {
        return None;
    }
    Some(v)
}

#[derive(Debug, Clone, PartialEq)]
enum CalcTok {
    Num(f64, String),
    Op(char),
    LParen,
    RParen,
}

fn tokenize_calc(expr: &str) -> Option<Vec<CalcTok>> {
    let b = expr.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '(' {
            out.push(CalcTok::LParen);
            i += 1;
            continue;
        }
        if c == ')' {
            out.push(CalcTok::RParen);
            i += 1;
            continue;
        }
        if c == '+' || c == '-' || c == '*' || c == '/' {
            out.push(CalcTok::Op(c));
            i += 1;
            continue;
        }
        // 数字(可带小数点)+ 可选单位。
        let start = i;
        while i < b.len() {
            let d = b[i] as char;
            if d.is_ascii_digit() || d == '.' {
                i += 1;
            } else {
                break;
            }
        }
        if i == start {
            return None;
        }
        let num: String = expr[start..i].to_string();
        let num: f64 = num.parse().ok()?;
        let unit_start = i;
        while i < b.len() {
            let u = b[i] as char;
            if u.is_ascii_alphabetic() || u == '%' {
                i += 1;
            } else {
                break;
            }
        }
        out.push(CalcTok::Num(num, expr[unit_start..i].to_string()));
    }
    Some(out)
}

/// calc 语法解析:表达式 → 项(乘除) → 因子(数字/括号/一元负)。
fn calc_expr(tokens: &[CalcTok], pos: &mut usize, font_size: f64, ctx: &ValueCtx) -> Option<f64> {
    let mut left = calc_term(tokens, pos, font_size, ctx)?;
    loop {
        let Some(CalcTok::Op(op)) = tokens.get(*pos) else { break };
        if *op != '+' && *op != '-' {
            break;
        }
        *pos += 1;
        let right = calc_term(tokens, pos, font_size, ctx)?;
        left = if *op == '+' { left + right } else { left - right };
    }
    Some(left)
}

fn calc_term(tokens: &[CalcTok], pos: &mut usize, font_size: f64, ctx: &ValueCtx) -> Option<f64> {
    let mut left = calc_factor(tokens, pos, font_size, ctx)?;
    loop {
        let Some(CalcTok::Op(op)) = tokens.get(*pos) else { break };
        if *op != '*' && *op != '/' {
            break;
        }
        *pos += 1;
        let right = calc_factor(tokens, pos, font_size, ctx)?;
        left = if *op == '*' {
            left * right
        } else {
            if right == 0.0 {
                return None;
            }
            left / right
        };
    }
    Some(left)
}

fn calc_factor(tokens: &[CalcTok], pos: &mut usize, font_size: f64, ctx: &ValueCtx) -> Option<f64> {
    match tokens.get(*pos)? {
        CalcTok::Num(n, unit) => {
            *pos += 1;
            calc_unit_to_px(*n, unit, font_size, ctx)
        }
        CalcTok::LParen => {
            *pos += 1;
            let v = calc_expr(tokens, pos, font_size, ctx)?;
            match tokens.get(*pos)? {
                CalcTok::RParen => {
                    *pos += 1;
                    Some(v)
                }
                _ => None,
            }
        }
        CalcTok::Op('-') => {
            *pos += 1;
            calc_factor(tokens, pos, font_size, ctx).map(|v| -v)
        }
        CalcTok::Op('+') => {
            *pos += 1;
            calc_factor(tokens, pos, font_size, ctx)
        }
        _ => None,
    }
}

/// calc 内部数字+单位 → px(`%` 沿用视口宽近似,见 eval_calc 注释)。
fn calc_unit_to_px(value: f64, unit: &str, font_size: f64, ctx: &ValueCtx) -> Option<f64> {
    match unit {
        "" | "px" => Some(value),
        "em" => Some(value * font_size),
        "rem" => Some(value * ctx.root_font_size),
        "%" => Some(value / 100.0 * ctx.viewport_width),
        "vw" => Some(value / 100.0 * ctx.viewport_width),
        "vh" => Some(value / 100.0 * ctx.viewport_height),
        "vmin" => Some(value / 100.0 * ctx.viewport_width.min(ctx.viewport_height)),
        "vmax" => Some(value / 100.0 * ctx.viewport_width.max(ctx.viewport_height)),
        "pt" => Some(value * 4.0 / 3.0),
        "pc" => Some(value * 16.0),
        "in" => Some(value * 96.0),
        "cm" => Some(value * 37.795),
        "mm" => Some(value * 3.7795),
        _ => None,
    }
}

fn parse_font_size(value: &str, parent_size: f64, ctx: &ValueCtx) -> Option<f64> {
    let v = value.trim().to_ascii_lowercase();
    let keyword = match v.as_str() {
        "xx-small" => Some(9.0),
        "x-small" => Some(10.0),
        "small" => Some(13.0),
        "medium" => Some(16.0),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        "smaller" => Some(parent_size * 0.85),
        "larger" => Some(parent_size * 1.2),
        "inherit" => Some(parent_size),
        _ => None,
    };
    if keyword.is_some() {
        return keyword;
    }
    parse_length(&v, parent_size, ctx)
}

fn parse_line_height(value: &str, font_size: f64, ctx: &ValueCtx) -> Option<f64> {
    let v = value.trim().to_ascii_lowercase();
    if v == "normal" {
        return Some(font_size * 1.45);
    }
    if let Some(n) = v.strip_suffix('%') {
        return n.trim().parse::<f64>().ok().map(|n| font_size * n / 100.0);
    }
    if let Ok(n) = v.parse::<f64>() {
        // 无单位:相对字号倍数。
        return Some(font_size * n);
    }
    parse_length(&v, font_size, ctx)
}

fn parse_font_weight(value: &str, current: u16) -> Option<u16> {
    let v = value.trim().to_ascii_lowercase();
    match v.as_str() {
        "normal" => Some(400),
        "bold" => Some(700),
        "bolder" => Some((current + 300).min(900)),
        "lighter" => Some(current.saturating_sub(300).max(100)),
        _ => v.parse::<u16>().ok().map(|n| n.clamp(100, 900)),
    }
}

/// 解析 1~4 个盒模型分量;含 `auto` 的按 0 处理(简化)。
fn parse_box_values(value: &str, font_size: f64, ctx: &ValueCtx) -> Vec<f64> {
    value
        .split_whitespace()
        .filter_map(|part| {
            if part.eq_ignore_ascii_case("auto") {
                Some(0.0)
            } else {
                parse_length(part, font_size, ctx)
            }
        })
        .collect()
}

fn expand_box(values: &[f64]) -> Option<(f64, f64, f64, f64)> {
    match values.len() {
        1 => Some((values[0], values[0], values[0], values[0])),
        2 => Some((values[0], values[1], values[0], values[1])),
        3 => Some((values[0], values[1], values[2], values[1])),
        4 => Some((values[0], values[1], values[2], values[3])),
        _ => None,
    }
}

// ===================== Flex / 定位辅助 =====================

/// 主轴对齐关键字归一(justify-content / align-content)。
fn normalize_flex_align(value: &str) -> Option<String> {
    let v = match value.trim().to_ascii_lowercase().as_str() {
        "start" | "left" => "flex-start",
        "end" | "right" => "flex-end",
        "flex-start" => "flex-start",
        "flex-end" => "flex-end",
        "center" => "center",
        "space-between" => "space-between",
        "space-around" => "space-around",
        "space-evenly" => "space-evenly",
        _ => return None,
    };
    Some(v.to_string())
}

/// 交叉轴对齐关键字归一(align-items)。
fn normalize_cross_align(value: &str) -> Option<String> {
    let v = match value.trim().to_ascii_lowercase().as_str() {
        "start" | "self-start" => "flex-start",
        "end" | "self-end" => "flex-end",
        "flex-start" => "flex-start",
        "flex-end" => "flex-end",
        "center" => "center",
        "stretch" => "stretch",
        "baseline" => "baseline",
        _ => return None,
    };
    Some(v.to_string())
}

/// Flex 域对齐关键字 → Grid 域(`flex-start`→`start`,`baseline` 按 `start`)。
fn to_grid_align(v: &str) -> String {
    match v {
        "flex-start" => "start".into(),
        "flex-end" => "end".into(),
        "baseline" => "start".into(),
        other => other.to_string(),
    }
}

/// Grid 对齐关键字(容器/条目通用;`normal` 归一为 `stretch`)。
fn normalize_grid_align(value: &str) -> Option<String> {
    let v = match value.trim().to_ascii_lowercase().as_str() {
        "normal" | "stretch" => "stretch",
        "start" | "flex-start" | "left" | "self-start" => "start",
        "end" | "flex-end" | "right" | "self-end" => "end",
        "center" => "center",
        _ => return None,
    };
    Some(v.to_string())
}

// ===================== Grid 轨道词法(阶段三第二步) =====================

/// 轨道列表:顶层按空白/逗号切分;`repeat(n, …)` 展开;无法识别的词丢弃。
pub fn parse_track_list(value: &str, font_size: f64, ctx: &ValueCtx) -> Vec<TrackSize> {
    let v = value.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    let mut out = Vec::new();
    for part in split_top_level(v) {
        expand_track_part(&part, font_size, ctx, &mut out);
    }
    out
}

/// 顶层切分:空白与逗号均可分隔;括号内递归不切。
fn split_top_level(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    for ch in s.chars() {
        if ch == '(' {
            depth += 1;
            cur.push(ch);
        } else if ch == ')' {
            depth = depth.saturating_sub(1);
            cur.push(ch);
        } else if depth == 0 && (ch == ',' || ch.is_whitespace()) {
            let t = cur.trim().to_string();
            if !t.is_empty() {
                parts.push(t);
            }
            cur.clear();
        } else {
            cur.push(ch);
        }
    }
    let t = cur.trim().to_string();
    if !t.is_empty() {
        parts.push(t);
    }
    parts
}

/// 展开单个列表项(`repeat` 递归展开;其余按单轨道尺寸解析)。
fn expand_track_part(part: &str, font_size: f64, ctx: &ValueCtx, out: &mut Vec<TrackSize>) {
    let lower = part.to_ascii_lowercase();
    if let Some(body) = lower
        .strip_prefix("repeat(")
        .and_then(|b| b.strip_suffix(')'))
    {
        let mut it = split_top_level(body).into_iter();
        // 次数整数;auto-fill/auto-fit 不支持(整段丢弃,见设计文档边界)。
        let count: usize = it
            .next()
            .and_then(|p| p.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let mut inner = Vec::new();
        for p in it {
            expand_track_part(p.trim(), font_size, ctx, &mut inner);
        }
        if count > 0 && count <= 64 && !inner.is_empty() {
            for _ in 0..count {
                out.extend(inner.iter().cloned());
            }
        }
        return;
    }
    if let Some(t) = parse_track_size(part, font_size, ctx) {
        out.push(t);
    }
}

/// 单个轨道尺寸。无法识别返回 `None`(宁缺勿错)。
pub fn parse_track_size(value: &str, font_size: f64, ctx: &ValueCtx) -> Option<TrackSize> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    let lower = v.to_ascii_lowercase();
    match lower.as_str() {
        "none" => return None,
        "auto" | "fit-content" => return Some(TrackSize::Auto),
        "min-content" => return Some(TrackSize::MinContent),
        "max-content" => return Some(TrackSize::MaxContent),
        _ => {}
    }
    if let Some(body) = lower
        .strip_prefix("minmax(")
        .and_then(|b| b.strip_suffix(')'))
    {
        let mut it = split_top_level(body).into_iter();
        let min = it.next().and_then(|p| parse_track_size(&p, font_size, ctx));
        let max = it.next().and_then(|p| parse_track_size(&p, font_size, ctx));
        if let (Some(min), Some(max)) = (min, max) {
            // 分量 fr 按 auto 处理(见设计文档边界)。
            let min = if matches!(min, TrackSize::Fr(_)) {
                TrackSize::Auto
            } else {
                min
            };
            let max = if matches!(max, TrackSize::Fr(_)) {
                TrackSize::Auto
            } else {
                max
            };
            return Some(TrackSize::MinMax(Box::new(min), Box::new(max)));
        }
        return None;
    }
    if lower.starts_with("fit-content(") {
        return Some(TrackSize::Auto);
    }
    if let Some(n) = lower.strip_suffix("fr") {
        return n
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| *v >= 0.0)
            .map(TrackSize::Fr);
    }
    if let Some(n) = lower.strip_suffix('%') {
        return n.trim().parse::<f64>().ok().map(TrackSize::Percent);
    }
    parse_length(v, font_size, ctx).map(TrackSize::Px)
}

/// 网格线值:`auto` / 整数(1 基) / `span N`;命名线等不支持。
fn parse_grid_line(value: &str) -> GridLine {
    let v = value.trim().to_ascii_lowercase();
    if v.is_empty() || v == "auto" {
        return GridLine::Auto;
    }
    if let Some(rest) = v.strip_prefix("span") {
        let n = rest.trim().parse::<u32>().ok().unwrap_or(1);
        return GridLine::Span(n.max(1));
    }
    if let Ok(n) = v.parse::<i64>() {
        return GridLine::Int(n);
    }
    GridLine::Auto
}

/// `grid-column`/`grid-row` 简写 `start / end`;省略 end 时 Int→+1、Span 保持。
fn parse_grid_line_shorthand(value: &str) -> (GridLine, GridLine) {
    let parts: Vec<&str> = value.split('/').collect();
    let start = parse_grid_line(parts.first().copied().unwrap_or("auto"));
    let end = match parts.get(1) {
        Some(s) => parse_grid_line(s),
        None => match start {
            GridLine::Int(n) => GridLine::Int(n + 1),
            GridLine::Span(n) => GridLine::Span(n),
            GridLine::Auto => GridLine::Auto,
        },
    };
    (start, end)
}

/// `flex` 简写解析:`none` → 0 0 auto;`auto`/`initial` → 1 1 auto;
/// 单数字 → `g 1 auto`;`g s` / `g s b` 组合。
fn apply_flex_shorthand(st: &mut ComputedStyle, value: &str, ctx: &ValueCtx) {
    let v = value.trim().to_ascii_lowercase();
    match v.as_str() {
        "none" => {
            st.flex_item.grow = 0.0;
            st.flex_item.shrink = 0.0;
            st.flex_item.basis = None;
            return;
        }
        "auto" | "initial" => {
            st.flex_item.grow = 1.0;
            st.flex_item.shrink = 1.0;
            st.flex_item.basis = None;
            return;
        }
        _ => {}
    }
    let parts: Vec<&str> = v.split_whitespace().collect();
    if parts.is_empty() {
        return;
    }
    if let Ok(g) = parts[0].parse::<f64>() {
        st.flex_item.grow = g.max(0.0);
    }
    if let Some(s) = parts.get(1) {
        if let Ok(sh) = s.parse::<f64>() {
            st.flex_item.shrink = sh.max(0.0);
        }
    }
    if let Some(b) = parts.get(2) {
        st.flex_item.basis = parse_length(b, st.font_size, ctx).filter(|v| *v > 0.0);
    }
}

// ===================== 颜色 =====================

/// 解析颜色。外层 `None` 表示无法识别;`Some(None)` 表示 `transparent`。
pub fn parse_color(value: &str) -> Option<Option<([u8; 3], u8)>> {
    let v = value.trim().to_ascii_lowercase();
    if v.is_empty() {
        return None;
    }
    if v == "transparent" {
        return Some(None);
    }
    if let Some(rgb) = named_color(&v) {
        return Some(Some((rgb, 255)));
    }
    if let Some(hex) = v.strip_prefix('#') {
        return parse_hex_color(hex).map(Some);
    }
    if let Some(rest) = v.strip_prefix("rgba(").or_else(|| v.strip_prefix("rgb(")) {
        let rest = rest.trim_end_matches(')');
        let parts: Vec<&str> = rest.split(',').map(str::trim).collect();
        if parts.len() >= 3 {
            let r = parse_channel(parts[0])?;
            let g = parse_channel(parts[1])?;
            let b = parse_channel(parts[2])?;
            let a = if let Some(raw) = parts.get(3) {
                let raw = raw.trim();
                let is_pct = raw.ends_with('%');
                let n = raw.trim_end_matches('%').parse::<f64>().ok()?;
                let n = if is_pct { n / 100.0 } else { n };
                (n.clamp(0.0, 1.0) * 255.0).round() as u8
            } else {
                255
            };
            return Some(Some(([r, g, b], a)));
        }
        return None;
    }
    if let Some(rest) = v.strip_prefix("hsla(").or_else(|| v.strip_prefix("hsl(")) {
        let rest = rest.trim_end_matches(')');
        let parts: Vec<&str> = rest.split(',').map(str::trim).collect();
        if parts.len() >= 3 {
            let h = parts[0].trim_end_matches("deg").parse::<f64>().ok()?;
            let s = parts[1].trim_end_matches('%').parse::<f64>().ok()? / 100.0;
            let l = parts[2].trim_end_matches('%').parse::<f64>().ok()? / 100.0;
            let a = parts
                .get(3)
                .and_then(|s| s.parse::<f64>().ok())
                .map(|a| (a.clamp(0.0, 1.0) * 255.0).round() as u8)
                .unwrap_or(255);
            return Some(Some((hsl_to_rgb(h, s, l), a)));
        }
    }
    None
}

fn resolve_color(value: &str, current: [u8; 3]) -> Option<[u8; 3]> {
    if value.trim().eq_ignore_ascii_case("currentcolor") {
        return Some(current);
    }
    parse_color(value)
        .and_then(|c| c)
        .map(|(rgb, a)| blend_on_white(rgb, a))
}

fn blend_on_white(c: [u8; 3], alpha: u8) -> [u8; 3] {
    if alpha == 255 {
        return c;
    }
    let a = alpha as f64 / 255.0;
    [
        (c[0] as f64 * a + 255.0 * (1.0 - a)).round() as u8,
        (c[1] as f64 * a + 255.0 * (1.0 - a)).round() as u8,
        (c[2] as f64 * a + 255.0 * (1.0 - a)).round() as u8,
    ]
}

fn parse_channel(s: &str) -> Option<u8> {
    let s = s.trim();
    if let Some(p) = s.strip_suffix('%') {
        let v = p.trim().parse::<f64>().ok()?;
        return Some((v / 100.0 * 255.0).round().clamp(0.0, 255.0) as u8);
    }
    s.parse::<f64>()
        .ok()
        .map(|v| v.round().clamp(0.0, 255.0) as u8)
}

fn parse_hex_color(hex: &str) -> Option<([u8; 3], u8)> {
    if !hex.is_ascii() {
        return None;
    }
    let chars: Vec<char> = hex.chars().filter(|c| !c.is_whitespace()).collect();
    let expand = |c: char| -> Option<u8> { c.to_digit(16).map(|d| (d * 17) as u8) };
    match chars.len() {
        3 => Some((
            [expand(chars[0])?, expand(chars[1])?, expand(chars[2])?],
            255,
        )),
        4 => Some((
            [expand(chars[0])?, expand(chars[1])?, expand(chars[2])?],
            expand(chars[3])?,
        )),
        6 => Some((
            [
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
            ],
            255,
        )),
        8 => Some((
            [
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
            ],
            u8::from_str_radix(&hex[6..8], 16).ok()?,
        )),
        _ => None,
    }
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> [u8; 3] {
    let h = h.rem_euclid(360.0) / 360.0;
    if s <= 0.0 {
        let v = (l * 255.0).round().clamp(0.0, 255.0) as u8;
        return [v, v, v];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hue = |mut t: f64| -> f64 {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [
        (hue(h + 1.0 / 3.0) * 255.0).round().clamp(0.0, 255.0) as u8,
        (hue(h) * 255.0).round().clamp(0.0, 255.0) as u8,
        (hue(h - 1.0 / 3.0) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

fn named_color(name: &str) -> Option<[u8; 3]> {
    Some(match name {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [255, 0, 0],
        "green" => [0, 128, 0],
        "blue" => [0, 0, 255],
        "yellow" => [255, 255, 0],
        "orange" => [255, 165, 0],
        "purple" => [128, 0, 128],
        "pink" => [255, 192, 203],
        "brown" => [165, 42, 42],
        "gray" | "grey" => [128, 128, 128],
        "lightgray" | "lightgrey" => [211, 211, 211],
        "darkgray" | "darkgrey" => [169, 169, 169],
        "silver" => [192, 192, 192],
        "gold" => [255, 215, 0],
        "cyan" | "aqua" => [0, 255, 255],
        "magenta" | "fuchsia" => [255, 0, 255],
        "lime" => [0, 255, 0],
        "navy" => [0, 0, 128],
        "teal" => [0, 128, 128],
        "olive" => [128, 128, 0],
        "maroon" => [128, 0, 0],
        "beige" => [245, 245, 220],
        "ivory" => [255, 255, 240],
        "khaki" => [240, 230, 140],
        "salmon" => [250, 128, 114],
        "tomato" => [255, 99, 71],
        "coral" => [255, 127, 80],
        "crimson" => [220, 20, 60],
        "indigo" => [75, 0, 130],
        "violet" => [238, 130, 238],
        "turquoise" => [64, 224, 208],
        "skyblue" => [135, 206, 235],
        "steelblue" => [70, 130, 180],
        "royalblue" => [65, 105, 225],
        "dodgerblue" => [30, 144, 255],
        "forestgreen" => [34, 139, 34],
        "seagreen" => [46, 139, 87],
        "darkgreen" => [0, 100, 0],
        "darkblue" => [0, 0, 139],
        "darkred" => [139, 0, 0],
        "transparent" => return None,
        _ => return None,
    })
}
