//! 行内格式化上下文:样式 run → 共享行盒的段落流(`flush_inline`)。
//!
//! 从 `layout/mod.rs` 机械拆分(单文件超 1800 行规范):`InlineBuf`/`InlineRun`
//! 为收集期缓冲模型,`Seg`/`flush_inline`/`commit_line` 为排布期行盒模型。
//! 同一视觉行可容纳多个样式段(x 连续流动、行高取段间最大值),换行点可在
//! run 边界或 run 内部;行首/行尾可折叠空白按行边裁剪。
//!
//! 第 44 轮:引入**行内原子盒**(替换元素与表单控件)—— 它们参与行宽、
//! 断行与基线对齐,详见 `inline_atomic.rs` 的模块文档。
//!
//! 第 47 轮(行内排版第二层):
//! - **行内块独立格式化上下文**:`display:inline-block`/`inline-flex` 的子代
//!   进**独立** `InlineBuf`,按盒内容宽换行、在盒内发射,盒高由内容决定
//!   (`layout_inline_block_ctx_at` + `flush_inline_floor`);
//! - **行内片段盒**:普通行内元素(`a`/`span`/`label`/…)的背景/边框/阴影由
//!   文本 run 携带的 `InlineOwner` 在**行提交期**按片段绘制
//!   (`emit_inline_fragment`),不再画在收集期的行首;
//! - **窄容器换行**:断行后清零待入段宽(`seg_w`)、词界回退断行。

use super::inline_atomic::{AtomicKind, InlineAtomic, VAlign};
use super::super::render::display::WidgetGlyphKind;
use super::super::render::display::DisplayKind;
use super::super::render::display::DisplayItem;
use super::super::style::CornerRadius;
use super::*;

/// 行内**片段盒**属主(第 47 轮):文本 run 的最内层普通行内元素。
///
/// 普通行内元素(`a`/`span`/`b`/`label`/…)的背景/边框/阴影**按行盒片段**
/// 逐段绘制(CSS 2.1 §10.6.1:行内非替换元素的背景覆盖其 inline box,
/// 多行时每行一段;左右内边距只在**首末片段**上各出现一次,上下内边距
/// 每段都有但不影响行高)。此前内核在**收集期**把盒画在行首,链接/标签的
/// 背景整块盖到前一个词上,多行时高度只有一行。
///
/// 只存绘制必需的快照(选择器/标签/计算样式),不持 `NodeRef` —— run 会被
/// 克隆进 `Seg`,持节点会让克隆变贵且与 `LayoutCtx` 的生命周期纠缠。
#[derive(Clone)]
pub(crate) struct InlineOwner {
    selector: String,
    tag: String,
    style: ComputedStyle,
}

impl PartialEq for InlineOwner {
    /// 合并判定只看**身份**:同一元素的相邻 run 合并后仍属同一片段盒属主,
    /// 样式差异由 run 自身的样式段承载。
    fn eq(&self, other: &Self) -> bool {
        self.selector == other.selector && self.tag == other.tag
    }
}

/// 行内格式化上下文中的一个样式段(同一视觉行可容纳多个段)。
struct Seg {
    text: String,
    /// 段首 x(行首段 = run.x;行中段 = 流点与 run.x 取大)。
    x: f64,
    w: f64,
    run: InlineRun,
}

/// 行盒状态(第 44 轮):行内流位置与基线分解。
///
/// 行盒高度按「基线以上 + 基线以下」两段计算(浏览器语义):文本段的
/// 基线以上高度取字号(渲染层把基线画在文字顶 + 字号处),基线以下取
/// `line-height - 字号`;原子盒按自身类别给出两段高度。
struct LineState {
    /// 已落的文本段。
    segs: Vec<Seg>,
    /// 已落的原子盒(`InlineBuf::atomics` 的下标 + 该盒的最终左边 x)。
    atomics: Vec<(usize, f64)>,
    /// 基线以上高度。
    ascent: f64,
    /// 基线以下高度。
    descent: f64,
    /// 行首 x(缩进基准)。
    origin_x: f64,
    /// 当前流位置(下一个行内项的落点)。
    flow_x: f64,
    /// 本行**首个** run 的收集期 x(累计 pen)。换行后行首回到容器左缘,
    /// run.x 需减掉它才得到「行内相对位置」(见 `flush_run`)。
    first_run_x: f64,
    /// 行是否已有内容。
    started: bool,
}

impl LineState {
    fn new() -> Self {
        Self {
            segs: Vec::new(),
            atomics: Vec::new(),
            ascent: 0.0,
            descent: 0.0,
            origin_x: 0.0,
            flow_x: 0.0,
            first_run_x: 0.0,
            started: false,
        }
    }

    /// 提交一行后复位(下一行从行首重新排)。
    fn reset(&mut self) {
        self.segs.clear();
        self.atomics.clear();
        self.ascent = 0.0;
        self.descent = 0.0;
        self.origin_x = 0.0;
        self.flow_x = 0.0;
        self.first_run_x = 0.0;
        self.started = false;
    }

    /// 行内容宽(`text-align` 居中/右对齐的基准)。
    fn width(&self) -> f64 {
        (self.flow_x - self.origin_x).max(0.0)
    }
}

#[derive(Default)]
pub(crate) struct InlineBuf {
    runs: Vec<InlineRun>,
    /// 行内原子盒(第 44 轮):替换元素与表单控件,参与行宽与基线计算。
    pub(super) atomics: Vec<InlineAtomic>,
    /// 第 48 轮:`mark` 空文本 run 的 (order, 期望宽度)。
    mark_widths: Vec<(u64, f64)>,
    /// 收集顺序计数器:文本 run 与原子盒共用,保证二者在行内的先后关系。
    order: u64,
    pub(super) chars: usize,
}

#[derive(Clone)]
struct InlineRun {
    text: String,
    x: f64,
    size: f64,
    color: [u8; 3],
    bold: bool,
    line_height: f64,
    visible: bool,
    align: String,
    wrap: bool,
    preserve: bool,
    /// 行内元素自身变换(块级文本为 `None`,由 `apply_group` 回补)。
    transform: Option<Affine>,
    /// 行内样式携带的 text-shadow 层。
    text_shadow: Vec<TextShadow>,
    /// 第 48 轮:文本装饰线。
    decor: super::TextDecor,
    /// 第 48 轮:语义高亮背景(`mark`)。
    highlight: Option<([u8; 3], u8)>,
    /// 收集顺序(与原子盒共用序号)。
    order: u64,
    /// 已「封口」:其后不允许再并入同风格文本(原子盒前后不得合并,
    /// 否则会丢掉原子盒在行内的先后位置)。
    sealed: bool,
    /// 行内片段盒属主(第 47 轮):该 run 的文本所属最内层普通行内元素。
    /// `None` = 块级文本或无属主(不画片段盒)。
    owner: Option<InlineOwner>,
}

impl InlineBuf {
    /// 下一个收集序号(文本 run 与原子盒共用)。
    fn next_order(&mut self) -> u64 {
        self.order += 1;
        self.order - 1
    }

    pub(crate) fn push_styled(
        &mut self,
        text: &str,
        st: &ComputedStyle,
        x_base: f64,
        transform: Option<Affine>,
        text_shadow: Vec<TextShadow>,
        owner: Option<InlineOwner>,
    ) {
        self.push_styled_decor(
            text,
            st,
            x_base,
            transform,
            text_shadow,
            owner,
            TextDecor::default(),
            None,
        );
    }

    /// 带装饰/高亮的文本 run 收集(第 48 轮)。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_styled_decor(
        &mut self,
        text: &str,
        st: &ComputedStyle,
        x_base: f64,
        transform: Option<Affine>,
        text_shadow: Vec<TextShadow>,
        owner: Option<InlineOwner>,
        decor: super::TextDecor,
        highlight: Option<([u8; 3], u8)>,
    ) {
        self.chars += text.chars().count();
        let size = st.font_size;
        let bold = st.font_weight >= 600;
        let visible = st.visibility != "hidden" && st.visibility != "collapse" && st.opacity > 0.0;
        let x = x_base + st.margin_left + st.padding_left;
        let preserve = matches!(st.white_space.as_str(), "pre" | "pre-wrap" | "break-spaces");
        let wrap = !matches!(st.white_space.as_str(), "pre" | "nowrap");
        // `white-space: normal|nowrap` 的换行/制表/连续空格折叠成单个空格
        // (CSS white-space 语义)。行内文本节点保留源码缩进换行,不折叠就会被
        // 当成硬换行 —— 京东页脚每个 `<a>` 之间被缩进换行切成独立行、页面被
        // 撑到 5777px;arXiv 主题列表同样逐行阶梯右移、整页宽溢出到 7473px。
        // (2026-10-30:此折叠在 `9340641` 合并冲突里被丢掉,`preserve` 字段留下
        // 而调用丢失,此处恢复。)
        let text = if preserve {
            text.to_string()
        } else {
            collapse_ws(text)
        };
        let order = self.next_order();
        if let Some(last) = self.runs.last_mut() {
            if !last.sealed
                && (last.size - size).abs() < 0.1
                && last.color == st.color
                && last.bold == bold
                && last.visible == visible
                && last.x == x
                && last.align == st.text_align
                && last.wrap == wrap
                && last.preserve == preserve
                && last.transform == transform
                && last.text_shadow == text_shadow
                && last.decor == decor
                && last.highlight == highlight
                && last.owner == owner
            {
                last.text.push_str(&text);
                return;
            }
        }
        self.runs.push(InlineRun {
            text: text.to_string(),
            x,
            size,
            color: st.color,
            bold,
            line_height: if st.line_height > 0.0 {
                st.line_height
            } else {
                size * 1.45
            },
            visible,
            align: st.text_align.clone(),
            wrap,
            preserve,
            transform,
            text_shadow,
            decor,
            highlight,
            order,
            sealed: false,
            owner,
        });
    }

    /// 登记一个行内原子盒(第 44 轮)。
    ///
    /// 原子盒**封口**其前的文本 run:后续同风格文本另起一段,否则合并会
    /// 丢掉「文本 → 原子盒 → 文本」的行内先后关系。
    pub(super) fn push_atomic(&mut self, mut atomic: InlineAtomic) {
        let order = self.next_order();
        if let Some(last) = self.runs.last_mut() {
            last.sealed = true;
        }
        atomic.order = order;
        self.atomics.push(atomic);
    }

    pub(super) fn force_break(&mut self) {
        if let Some(last) = self.runs.last_mut() {
            last.text.push('\n');
            return;
        }
        // 行首 `<br>`(此前无 run 可追加):插入一个只含换行的占位 run,
        // 否则 `flush_run` 的换行分支拿不到任何东西,换行被静默吞掉。
        let order = self.next_order();
        self.runs.push(InlineRun {
            text: "\n".to_string(),
            x: 0.0,
            size: 8.0,
            color: [0, 0, 0],
            bold: false,
            line_height: 0.0,
            visible: true,
            align: "left".to_string(),
            wrap: false,
            preserve: false,
            transform: None,
            text_shadow: Vec::new(),
            decor: super::TextDecor::default(),
            highlight: None,
            order,
            sealed: false,
            owner: None,
        });
    }

    /// 预留行高(行内盒撑开所在行)。
    ///
    /// 无可用 run 时插入一个空文本占位 run,`flush_inline` 只按其高度推进游标。
    pub(crate) fn reserve_height(&mut self, h: f64) {
        if h <= 0.0 {
            return;
        }
        if let Some(last) = self.runs.last_mut() {
            last.line_height = last.line_height.max(h);
            return;
        }
        let order = self.next_order();
        self.runs.push(InlineRun {
            text: String::new(),
            x: 0.0,
            size: 8.0,
            color: [0, 0, 0],
            bold: false,
            line_height: h,
            visible: true,
            align: "left".to_string(),
            wrap: false,
            preserve: false,
            transform: None,
            text_shadow: Vec::new(),
            decor: super::TextDecor::default(),
            highlight: None,
            order,
            sealed: false,
            owner: None,
        });
    }

    /// 第 48 轮:`mark` 空文本高亮 run —— 只发射背景,不产生文字。
    pub(crate) fn push_mark(&mut self, st: &ComputedStyle, x_base: f64, w: f64) {
        let order = self.next_order();
        self.runs.push(InlineRun {
            text: String::new(),
            x: x_base + st.margin_left + st.padding_left,
            size: st.font_size.max(8.0),
            color: st.color,
            bold: st.font_weight >= 600,
            line_height: if st.line_height > 0.0 {
                st.line_height
            } else {
                st.font_size * 1.45
            },
            visible: true,
            align: st.text_align.clone(),
            wrap: false,
            preserve: false,
            transform: None,
            text_shadow: Vec::new(),
            decor: super::TextDecor::default(),
            highlight: Some(([255, 235, 130], 235)),
            order,
            sealed: false,
            owner: None,
        });
        // 记录本 mark run 的零宽占位:flush 时按给定宽度画高亮背景。
        self.mark_widths.push((order, w));
    }
}

/// 行内**块容器**(第 47 轮):`display:inline-block` / `inline-flex`。
///
/// 这类元素是行内级盒,但**建立独立格式化上下文**(BFC):子代不得漏进父行,
/// 盒高由内容决定。`inline-grid`/`inline-table` 在样式层被归一成
/// `grid`/`table`(见 `style::values::normalize_display`),布局层无法识别,
/// 仍走老的单盒近似 —— 列为已知边界。
fn is_inline_block_container(style: &ComputedStyle) -> bool {
    matches!(style.display.as_str(), "inline-block" | "inline-flex")
}

/// **普通行内元素**(第 47 轮):`display:inline` 且无显式尺寸。
///
/// 这类元素生成行内盒(不是块容器),其背景/边框必须**按行盒片段**绘制。
/// `display:inline` 携带显式尺寸是无效 CSS 组合,仍按老的单盒近似处理。
fn is_plain_inline(style: &ComputedStyle) -> bool {
    style.display == "inline"
        && style.width.is_none()
        && style.height.is_none()
        && style.width_percent.is_none()
}

/// 行内元素是否**绘制**了盒视觉(背景/背景层/边框/外阴影)。
fn inline_paints(style: &ComputedStyle) -> bool {
    style.background.is_some()
        || !style.background_layers.is_empty()
        || style
            .border_widths
            .iter()
            .zip(style.border_drawn.iter())
            .any(|(w, d)| *w > 0.0 && *d)
        || !style.box_shadow.is_empty()
}

/// `white-space: normal|nowrap` 的空白折叠:连续空白(空格/换行/制表)压成
/// 单个空格。行内文本节点保留源码缩进换行,不折叠就会被 `flush_run` 当成
/// 硬换行 —— 每个源码换行各起一行,且行首 origin 取累计 pen,逐行阶梯右移、
/// 整段溢出容器右缘(京东页脚 5777px、arXiv 主题列表 7473px 实测)。
fn collapse_ws(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_ws = false;
    for ch in text.chars() {
        // CSS 可折叠空白只含 ASCII 空格/制表/换行/回车/换页;不折叠 NBSP。
        if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{000C}') {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(ch);
            in_ws = false;
        }
    }
    out
}

impl<'a> LayoutCtx<'a> {
    /// 行内格式化上下文:把缓冲的样式 run 与原子盒排成**共享行盒**的段落流。
    ///
    /// 此前每个 run 各占一行竖排(`<p>a<b>b</b>c</p>` 渲染成三行),
    /// 行内样式混合(b/span/a/颜色)一律错位 —— 现按 CSS 行内语义:
    /// 同一视觉行可容纳多个样式段,段宽连续累加,行高取段间最大值,
    /// 换行点可在 run 边界或 run 内部,行首/行尾可折叠空白按行边裁剪。
    ///
    /// 第 44 轮:文本 run 与原子盒按**收集顺序**合并成一条事件流,
    /// 原子盒占宽并触发断行,行提交时按基线定位。
    pub(super) fn flush_inline(&mut self, buf: &mut InlineBuf, max_w: f64) {
        self.flush_inline_floor(buf, max_w, 80.0)
    }

    /// 带**换行下限**的行内 flush(第 47 轮)。
    ///
    /// 行内块容器的独立格式化上下文按**盒内容宽**换行,不能套用常规行内的
    /// 「不低于 80px」老下限 —— 否则 60px 宽的药丸标签里的文字会按 80px
    /// 换行、溢出盒外(正是「盒宽包住文字」要消灭的那类显示不全)。
    pub(super) fn flush_inline_floor(&mut self, buf: &mut InlineBuf, max_w: f64, floor: f64) {
        let max_w = max_w.max(floor);
        let items: &InlineBuf = buf;
        // 容器内容左缘(缩进基准)= 缓冲里**首个非空 run** 的收集期 x。
        // 换行后行首必须回到这里:run.x 是「不换行」时的**累计 pen**,
        // 直接当行首会让每行起点逐行右移(阶梯),整段溢出容器右缘。
        let base_x = items
            .runs
            .iter()
            .find(|r| !r.text.is_empty())
            .map(|r| r.x)
            .unwrap_or(0.0);
        let mut line = LineState::new();
        let mut ri = 0usize;
        let mut ai = 0usize;
        while ri < items.runs.len() || ai < items.atomics.len() {
            let run_first = ai >= items.atomics.len()
                || (ri < items.runs.len() && items.runs[ri].order < items.atomics[ai].order);
            if run_first {
                let run = items.runs[ri].clone();
                ri += 1;
                self.flush_run(&run, items, &mut line, max_w, base_x);
            } else {
                let idx = ai;
                ai += 1;
                self.flush_atomic(idx, items, &mut line, max_w);
            }
        }
        self.commit_line(&mut line, max_w, items);
        // 缓冲已排空:文本 run、原子盒与 mark 宽度都消费掉(否则下一次 flush 会重复排布)。
        buf.runs.clear();
        buf.atomics.clear();
        buf.mark_widths.clear();
        buf.chars = 0;
    }

    /// 把一个文本 run 排入当前行(必要时断行)。
    ///
    /// `base_x` = 容器内容左缘(缓冲首个非空 run 的 x)。行首一律回到这里,
    /// run.x 只用来算**行内相对位置**(见 `seg_x`)。
    fn flush_run(
        &mut self,
        run: &InlineRun,
        items: &InlineBuf,
        line: &mut LineState,
        max_w: f64,
        base_x: f64,
    ) {
        // 空文本占位 run:预留行高;若带 mark 高亮,仍按登记宽度画背景。
        if run.text.is_empty() {
            line.ascent = line.ascent.max(run.size);
            line.descent = line.descent.max((run.line_height - run.size).max(0.0));
            if let Some((_, w)) = items
                .mark_widths
                .iter()
                .find(|(order, _)| *order == run.order)
                .copied()
            {
                line.segs.push(Seg {
                    text: String::new(),
                    x: run.x,
                    w,
                    run: run.clone(),
                });
                line.flow_x = run.x + w;
            }
            return;
        }
        let size = run.size.max(8.0);
        let seg_line_h = run.line_height.max(size * 1.35);
        let mut seg_text = String::new();
        let mut seg_w = 0.0f64;
        let mut seg_x = 0.0f64;
        // 第 47 轮:字符源改为**可回插的队列** —— 词界回退时需要把已消耗的
        // 字符交还给循环(词后的字符移到下一行,而不是留在本行末尾)。
        let mut pending: std::collections::VecDeque<char> = run.text.chars().collect();
        while let Some(ch) = pending.pop_front() {
            if ch == '\n' {
                if !seg_text.is_empty() {
                    self.push_seg(
                        line,
                        run,
                        std::mem::take(&mut seg_text),
                        seg_x,
                        seg_w,
                        size,
                        seg_line_h,
                    );
                }
                self.commit_line(line, max_w, items);
                line.reset();
                // 第 47 轮:换行后**必须**清零待入段宽,否则下一行的断行判定
                // 会拿上一行的累计宽当基数 —— 窄容器(药丸标签、侧栏、代码块)
                // 的文字从第二行起逐字断行,整段不可读。
                seg_w = 0.0;
                continue;
            }
            let w = self.font.char_width(ch, size);
            // 跨段换行判定:整行(已占宽 + 本段待入宽)超可用宽即可断,
            // 不再限定「同一 run 内」——run 边界同样可断行。
            if run.wrap
                && line.started
                && line.flow_x - line.origin_x + seg_w + w > max_w
                && (!line.segs.is_empty() || !seg_text.is_empty())
            {
                // 第 47 轮:**词界回退**。浏览器在词界断行,不在词中截断 ——
                // 回退到本段最后一个空白,把其后的字符交还队列(下一行重排)。
                // `pre`/`nowrap` 不做回退(前者不换行,后者不断)。
                let mut tail = String::new();
                if !run.preserve {
                    if let Some(k) = seg_text.rfind(char::is_whitespace) {
                        if k > 0 {
                            tail = seg_text[k..].trim_start().to_string();
                            seg_text.truncate(k);
                            seg_w = self.font.text_width(&seg_text, size);
                        }
                    }
                }
                if !seg_text.is_empty() {
                    self.push_seg(
                        line,
                        run,
                        std::mem::take(&mut seg_text),
                        seg_x,
                        seg_w,
                        size,
                        seg_line_h,
                    );
                }
                self.commit_line(line, max_w, items);
                line.reset();
                // 第 47 轮:同上 —— 断行后清零待入段宽(见上处注释)。
                seg_w = 0.0;
                // 交还队列:触发断行的字符属于**下一行**,排在回退尾巴之后。
                // `push_front` 逆序插入,故按「尾巴 + 本字符」的逆序压。
                let mut carry: Vec<char> = tail.chars().collect();
                carry.push(ch);
                for c in carry.into_iter().rev() {
                    pending.push_front(c);
                }
                continue;
            }
            if seg_text.is_empty() {
                // 段落点:行首回到**容器内容左缘**(不再取累计 pen)。
                // run.x 是「不换行」时的累计位置,减掉本行首 run 的 x 才得到
                // 行内相对位置 —— 否则换行后行首逐行右移(阶梯),整段溢出容器。
                if !line.started {
                    line.origin_x = base_x;
                    line.flow_x = base_x;
                    line.first_run_x = run.x;
                    line.started = true;
                }
                let rel = line.origin_x + (run.x - line.first_run_x);
                seg_x = line.flow_x.max(rel);
            }
            seg_text.push(ch);
            seg_w += w;
        }
        if !seg_text.is_empty() && (run.preserve || !seg_text.trim().is_empty()) {
            self.push_seg(line, run, seg_text, seg_x, seg_w, size, seg_line_h);
        }
    }

    /// 落一个文本段到当前行(推进流位置并累计基线分解)。
    #[allow(clippy::too_many_arguments)]
    fn push_seg(
        &mut self,
        line: &mut LineState,
        run: &InlineRun,
        text: String,
        x: f64,
        w: f64,
        size: f64,
        seg_line_h: f64,
    ) {
        line.segs.push(Seg {
            text,
            x,
            w,
            run: run.clone(),
        });
        line.flow_x = x + w;
        // 基线以上 = 字号(渲染层把基线画在文字顶 + 字号处);
        // 基线以下 = line-height - 字号(行盒的下半段)。
        line.ascent = line.ascent.max(size);
        line.descent = line.descent.max((seg_line_h - size).max(0.0));
    }

    /// 把一个行内原子盒排入当前行:占宽、触发断行、累计基线分解。
    fn flush_atomic(&mut self, idx: usize, items: &InlineBuf, line: &mut LineState, max_w: f64) {
        let a = &items.atomics[idx];
        let occupied = a.occupied_width();
        // 放不下先断行(与文本同一套断行规则)。
        if line.started && line.flow_x - line.origin_x + occupied > max_w {
            self.commit_line(line, max_w, items);
            line.reset();
        }
        if !line.started {
            line.origin_x = a.base_x;
            line.flow_x = a.base_x;
            line.started = true;
        }
        // 盒左边 = 流位置 + 外边距 + 内边距(内核约定内边距在盒外)。
        let bx = line.flow_x + a.margin_left + a.padding_left;
        line.flow_x += occupied;
        line.atomics.push((idx, bx));
        let (asc, desc) = a.line_contribution();
        line.ascent = line.ascent.max(asc);
        line.descent = line.descent.max(desc);
    }

    /// 提交一个视觉行:段落逐个发射文本项(共用行首 y),游标推进一次。
    /// 行内全部段落被裁剪为空(non-preserve)时整行不落地也不推进。
    ///
    /// 第 44 轮:行盒按**基线**组织 —— 基线位置 = 行顶 + 基线以上高度,
    /// 文本段按「基线 - 字号」落位,原子盒按各自的对齐方式落位。
    fn commit_line(&mut self, line: &mut LineState, max_w: f64, items: &InlineBuf) {
        if line.segs.is_empty() && line.atomics.is_empty() && line.ascent <= 0.0 {
            line.ascent = 0.0;
            line.descent = 0.0;
            return;
        }
        // 行盒落地前收编前置 pending(行盒与外边距不折叠:前块 margin_bottom
        // 在此完整落地,后块 margin_top 从行盒另起)。
        self.y += self.pending_margin_bottom;
        self.pending_margin_bottom = 0.0;
        // 同行共基线:基线 = 行顶 + 基线以上高度(最大字号或更高的原子盒)。
        let baseline = self.y + line.ascent;
        let n = line.segs.len();
        let mut emitted = 0usize;
        for (i, seg) in line.segs.iter().enumerate() {
            let content = if seg.run.preserve {
                seg.text.trim_end_matches('\n').to_string()
            } else if i == 0 {
                seg.text.trim_start().to_string()
            } else if i + 1 == n {
                seg.text.trim_end().to_string()
            } else {
                seg.text.clone()
            };
            if content.is_empty() && seg.run.highlight.is_none() {
                continue;
            }
            emitted += 1;
            // 片段盒宽按**裁剪后**的文本量折算(与实际绘制宽度一致)。
            let frag_w = self.font.text_width(&content, seg.run.size);
            let offset = match seg.run.align.as_str() {
                // 内在尺寸(max-content)测量语境:对齐偏移不参与宽度计算,
                // 否则居中文字的行起点会落到 ~5 万处,被当成内容宽。
                _ if self.intrinsic => 0.0,
                "center" => ((max_w - line.width()) / 2.0).max(0.0),
                "right" => (max_w - line.width()).max(0.0),
                _ => 0.0,
            };
            let text_line = TextLine {
                text: content,
                x: seg.x + offset,
                // 基线对齐:文字顶 = 基线 - 字号(渲染层再加字号得基线)。
                y: baseline - seg.run.size,
                size: seg.run.size,
                color: seg.run.color,
                bold: seg.run.bold,
                visible: seg.run.visible,
                decor: seg.run.decor,
                highlight: seg.run.highlight,
            };
            // 阶段六:文本显示项(flush 时取新 seq → 与前后块级兄弟正确分层)。
            self.emit_text(&text_line, seg.run.transform.clone(), &seg.run.text_shadow);
            self.lines.push(text_line);
            // 行内片段盒(第 47 轮):普通行内元素的背景/边框/阴影按**本片段**
            // 绘制 —— 位置与高度都是行提交后的最终值,不再画在收集期的行首。
            if let Some(owner) = &seg.run.owner {
                let first = !line.segs[..i]
                    .iter()
                    .any(|s| s.run.owner.as_ref() == Some(owner));
                let last = !line.segs[i + 1..]
                    .iter()
                    .any(|s| s.run.owner.as_ref() == Some(owner));
                self.emit_inline_fragment(
                    owner,
                    seg.x + offset,
                    frag_w,
                    baseline - seg.run.size,
                    seg.run.line_height,
                    first,
                    last,
                    seg.run.transform.clone(),
                );
            }
        }
        // 原子盒:按基线定位并发射(盒与显示项在此才产生,位置是最终位置)。
        for (idx, bx) in line.atomics.clone() {
            let a = &items.atomics[idx];
            let ay = match a.valign {
                VAlign::Baseline => baseline - a.ascent,
                // middle:盒中心与「基线 + 半个 x 高度」对齐(x 高度按字号近似)。
                VAlign::Middle => baseline + 0.25 * a.style.font_size - a.h / 2.0,
                // top:盒顶与行盒顶对齐;bottom:盒底与行盒底对齐。
                VAlign::Top => self.y,
                VAlign::Bottom => baseline + line.descent - a.h,
                VAlign::Offset(d) => baseline - a.ascent + d,
            };
            self.emit_atomic(a, bx, ay);
        }
        if emitted > 0 || !line.atomics.is_empty() || line.ascent > 0.0 {
            // 行盒高度 = 基线以上 + 基线以下。
            self.y += line.ascent + line.descent;
        }
        line.segs.clear();
        line.atomics.clear();
        line.ascent = 0.0;
        line.descent = 0.0;
    }

    /// 发射一个**行内片段盒**(第 47 轮):普通行内元素的背景/边框/阴影。
    ///
    /// 几何口径(CSS 2.1 §10.6.1 的行内非替换元素):
    /// - 左右内边距只在**首末片段**各出现一次(`first`/`last` 标志);
    /// - 上下内边距**每段都有**,但**不影响行高**(行盒高度已由 run 的
    ///   `line-height` 决定,垂直内边距只是让背景上下溢出);
    /// - 多行时每行一段,每段画完整四边 —— 与浏览器「逐行片段续画边框」一致。
    ///
    /// 无绘制内容(无背景/边框/阴影)时只落**几何寻址盒**,不产显示项。
    #[allow(clippy::too_many_arguments)]
    fn emit_inline_fragment(
        &mut self,
        owner: &InlineOwner,
        x: f64,
        w: f64,
        text_top: f64,
        line_height: f64,
        first: bool,
        last: bool,
        tf: Option<Affine>,
    ) {
        let st = &owner.style;
        let pad_l = if first { st.padding_left } else { 0.0 };
        let pad_r = if last { st.padding_right } else { 0.0 };
        let box_x = x - pad_l;
        let box_w = (w + pad_l + pad_r).max(0.0);
        let box_h = (st.padding_top + line_height + st.padding_bottom).max(0.0);
        let box_y = text_top - st.padding_top;
        if box_w <= 0.0 || box_h <= 0.0 {
            return;
        }
        let seq = self.next_seq();
        self.boxes.push(ElementBox {
            selector: owner.selector.clone(),
            tag: owner.tag.clone(),
            x: box_x,
            y: box_y,
            w: box_w,
            h: box_h,
            background: st.background,
            background_alpha: st.background_alpha,
            visible: style_visible(st),
            z_index: st.pos.z_index,
            out_of_flow: false,
        });
        push_border_and_shadow(self, st, box_x, box_y, box_w, box_h);
        self.emit_element_tf(seq, st, &owner.tag, box_x, box_y, box_w, box_h, tf);
    }

    /// 发射一个行内原子盒(替换元素 / 表单控件)。
    ///
    /// 第 44 轮:原子盒在**行提交时**才产生盒与显示项 —— 此时它的最终
    /// 位置(行内流位置 + 基线对齐)已经确定,不再需要事后平移。
    pub(super) fn emit_atomic(&mut self, a: &InlineAtomic, x: f64, y: f64) {
        let node = &a.node;
        let style = &a.style;
        let tag = document::tag_name(node);
        let (w, h) = (a.w, a.h);
        // 变换合成:自身变换前乘祖先链(行内元素的祖先变换在提交期才合成)。
        let mut tf = a.tf;
        if let Some(m) = self.own_transform(style, x, y, w, h) {
            tf = Some(match tf {
                Some(anc) => m.compose(&anc),
                None => m,
            });
        }
        let seq = self.next_seq();
        self.boxes.push(ElementBox {
            selector: Document::stable_selector(node),
            tag: tag.clone(),
            x,
            y,
            w,
            h,
            background: style.background,
            background_alpha: style.background_alpha,
            visible: style_visible(style),
            z_index: style.pos.z_index,
            out_of_flow: false,
        });
        // 输入框/按钮等交互元素的边框与外阴影(绘制不占几何)。
        push_border_and_shadow(self, style, x, y, w, h);
        self.emit_element(seq, style, &tag, x, y, w, h);
        self.emit_canvas_pixels(node, style, seq, x, y, w, h);
        match a.kind {
            AtomicKind::Replaced => {
                // 替换元素的内容图:按固有尺寸与 `object-fit` 贴图。
                if let Some(src) = self.image_display_src(node) {
                    if self.image_loaded(&src) {
                        let paint = PaintBox {
                            src,
                            x: x + style.padding_left,
                            y: y + style.padding_top,
                            w: (w - style.padding_left - style.padding_right).max(0.0),
                            h: (h - style.padding_top - style.padding_bottom).max(0.0),
                            fit: style.object_fit.clone(),
                            kind: PaintKind::Content,
                            visible: style_visible(style),
                            z_index: style.pos.z_index,
                            off_x: 0.0,
                            off_y: 0.0,
                        };
                        self.emit_image(&paint, style.opacity, tf);
                        self.paints.push(paint);
                    }
                }
            }
            AtomicKind::Control => {
                // 阶段七:表单控件的 value/placeholder 文本与内置 UI。
                widgets::emit_form_control_text(self, node, style, &tag, x, y, w, h, tf);
            }
        }
    }

    /// 普通行内元素的**片段盒属主**(第 47 轮)。
    ///
    /// 命中条件:`display:inline` 且无显式尺寸(**普通行内盒**,不是块容器),
    /// 并且要么**绘制了盒视觉**(背景/背景层/边框/外阴影),要么**需要几何
    /// 寻址**(`inline_box_worthy`:交互元素/带 id/显式尺寸)—— 后者即便不画
    /// 任何像素,也要保住 `rect_for` 的寻址盒(空锚点 `<a name=x>` 是
    /// 百度地图顶部条带根因的回归点)。
    ///
    /// 命中失败的普通行内元素(如纯文本 `<b>`/`<i>`)不携带属主:它们不画盒、
    /// 也不需要逐元素寻址,背景缺失是样式层未建模的属性(斜体/下划线等),
    /// 不在本轮范围。
    fn inline_fragment_owner(&self, node: &NodeRef, style: &ComputedStyle) -> Option<InlineOwner> {
        if !is_plain_inline(style) {
            return None;
        }
        if !inline_paints(style) && !self.inline_box_worthy(node, style) {
            return None;
        }
        Some(InlineOwner {
            selector: Document::stable_selector(node),
            tag: document::tag_name(node),
            style: style.clone(),
        })
    }

    /// 行内块容器子树的 **max-content 宽**估算(第 47 轮)。
    ///
    /// 沿 DOM 子树按「行内累加」:文本按字号折算,替换元素取固有宽,显式宽
    /// 直接采用,控件取 `inline_box_size` 口径,其余递归。**不铺开布局、
    /// 不发射任何几何**,供 shrink-to-fit 的 preferred 宽使用。
    fn inline_container_max_content(&self, node: &NodeRef, style: &ComputedStyle) -> f64 {
        self.inline_container_max_content_depth(node, style, 0)
    }

    /// `inline_container_max_content` 的递归体(`depth` 为防御性深度上限)。
    fn inline_container_max_content_depth(
        &self,
        node: &NodeRef,
        style: &ComputedStyle,
        depth: usize,
    ) -> f64 {
        if depth > 24 {
            return 0.0;
        }
        let mut acc = 0.0f64;
        for child in node.children() {
            if let Some(text) = child.as_text() {
                let raw = text.borrow();
                if !raw.trim().is_empty() {
                    acc += self.font.text_width(&raw, style.font_size);
                }
                continue;
            }
            if child.as_element().is_none() {
                continue;
            }
            let cs = self.engine.computed(&child, style);
            if cs.display == "none" || cs.suppress_children {
                continue;
            }
            let margins =
                cs.margin_left + cs.margin_right + cs.padding_left + cs.padding_right;
            let tag = document::tag_name(&child).to_ascii_lowercase();
            // 替换元素:固有宽(百分比宽按 auto,与 `replaced_size` 口径一致)。
            if matches!(tag.as_str(), "img" | "svg" | "canvas" | "iframe") {
                let (w, _h, _src) = self.replaced_size(&child, &cs, 0.0, None);
                acc += w + margins;
                continue;
            }
            if let Some(w) = cs.width {
                acc += w + margins;
                continue;
            }
            if is_form_control_tag(&child) {
                let (w, _h) = self.inline_box_size(&child, &cs, 0.0);
                acc += w + margins;
                continue;
            }
            if tag == "br" {
                continue;
            }
            acc += self.inline_container_max_content_depth(&child, &cs, depth + 1) + margins;
        }
        acc
    }

    /// 在 `(bx, by)` 处以内容宽 `cw` 布局**行内块容器**的子代并发射盒自身
    /// (第 47 轮)。返回 `(内容底, 盒高)`;全局游标复原(推进由调用方决定)。
    ///
    /// 子代进**独立行内缓冲**,按 `cw` 换行并在盒内发射 —— 不再漏进父行、
    /// 也不再让多行内容溢出盒外。盒高由内容决定(`height` 缺省时),显式高优先。
    ///
    /// 盒视觉(背景/边框/阴影/圆角)与几何寻址盒在此发射;**seq 先于子代预留**,
    /// 保证盒画在子代内容之下。
    #[allow(clippy::too_many_arguments)]
    fn layout_inline_block_ctx_at(
        &mut self,
        node: &NodeRef,
        style: &ComputedStyle,
        bx: f64,
        by: f64,
        cw: f64,
        tf: Option<Affine>,
        depth: usize,
    ) -> (f64, f64) {
        let box_seq = self.next_seq();
        let mut child_buf = InlineBuf::default();
        let mut inner_row = float::InlineRow::default();
        let saved_y = self.y;
        self.y = by;
        let inner_x = bx - style.margin_left - style.padding_left;
        self.collect_inline_children(
            node,
            style,
            &mut child_buf,
            depth,
            inner_x,
            cw,
            &mut inner_row,
            tf,
        );
        // 换行下限取 8px:行内块容器按**盒内容宽**换行,不能套用常规行内的
        // 80px 下限(否则 60px 宽的药丸里的文字按 80px 换行、溢出盒外)。
        self.flush_inline_floor(&mut child_buf, cw, 8.0);
        let mut content_bottom = self.y;
        let float_bottom = inner_row.content_bottom();
        if float_bottom > content_bottom {
            content_bottom = float_bottom;
        }
        self.y = saved_y;
        let content_h = (content_bottom - by).max(0.0);
        let h = constrain_height(
            style,
            style.height.map(|h| h.max(0.0)).unwrap_or(content_h),
        )
        .max(1.0);
        self.boxes.push(ElementBox {
            selector: Document::stable_selector(node),
            tag: document::tag_name(node),
            x: bx,
            y: by,
            w: cw,
            h,
            background: style.background,
            background_alpha: style.background_alpha,
            visible: style_visible(style),
            z_index: style.pos.z_index,
            out_of_flow: false,
        });
        push_border_and_shadow(self, style, bx, by, cw, h);
        let tag = document::tag_name(node);
        self.emit_element_tf(box_seq, style, &tag, bx, by, cw, h, tf);
        (content_bottom, h)
    }

    /// 行内级元素是否需要生成近似矩形(供 inspect/坐标点击/截图裁剪寻址)。
    ///
    /// 行内元素在 CSS 中生成行内盒而非块级盒,但 Agent 需要几何来定位与点击,
    /// 故对"可寻址"的行内元素各生成一个近似矩形:交互元素、`inline-block`、
    /// 以及带显式尺寸的行内元素。
    pub(super) fn inline_box_worthy(&self, node: &NodeRef, style: &ComputedStyle) -> bool {
        let tag = document::tag_name(node);
        if matches!(
            tag.as_str(),
            "a" | "button" | "input" | "select" | "textarea" | "label" | "progress" | "meter"
        ) {
            return true;
        }
        style.display == "inline-block"
            || style.display == "inline-flex"
            || style.width.is_some()
            || style.height.is_some()
            || style.width_percent.is_some()
            // 第 174 轮:带 id 的行内元素一律可寻址 —— id 是 Agent 侧最常用的
            // 选择器锚点,猎聘登录入口 span#header-quick-menu-login 是纯
            // display:inline 无显式尺寸,不在此列时快照矩形恒 0×0、点击坐标
            // 落在 (1,1) 死区(近似矩形不影响流,只服务几何寻址)。
            || document::attr(node, "id").map(|v| !v.trim().is_empty()).unwrap_or(false)
    }

    /// 行内盒的近似尺寸:显式 CSS 尺寸优先,其次按文本量估算。
    ///
    /// 百分比宽度按包含块宽度(`avail_w`)求解;百分比高度在父高未知时按 `auto`
    /// 处理(CSS 中行内元素的百分比高度同样不生效)。
    pub(super) fn inline_box_size(
        &self,
        node: &NodeRef,
        style: &ComputedStyle,
        avail_w: f64,
    ) -> (f64, f64) {
        // 第 175 轮:空行内元素(无文本、无显式尺寸、无可渲染子元素)量成 0×0 ——
        // 不产生行盒(Chrome 语义);盒照常发射,保住第 174 轮的几何寻址诉求
        // (Chrome 空行内元素 rect 亦为 0×0)。表单控件/替换类标签自身即有
        // 固有尺寸(void 元素无子节点,不能按「无子内容」判空)。
        let explicit_dim =
            style.width.is_some() || style.height.is_some() || style.width_percent.is_some();
        let self_replaced = matches!(
            document::tag_name(node).to_ascii_lowercase().as_str(),
            "input" | "select" | "textarea" | "button" | "img" | "svg" | "iframe" | "canvas"
                | "object" | "embed" | "video" | "audio" | "hr" | "progress" | "meter"
        );
        if !self_replaced && !explicit_dim && !has_structural_inline_content(node, 0) {
            return (0.0, 0.0);
        }
        let tag = document::tag_name(node).to_ascii_lowercase();
        let ty = document::attr(node, "type")
            .unwrap_or_default()
            .to_ascii_lowercase();
        // select 的 option 子树不参与布局;固有宽只看当前显示项 + 箭头位,
        // 否则 hidden option 的累计文本会把盒撑到页面外。
        let text = if tag == "select" {
            select_display_text(node)
        } else if tag == "input" {
            // input 是 void 元素,`text_contents()` 恒空;固有宽必须看
            // value/placeholder/按钮默认文案,否则长提示只剩 40px 窄盒后被截断。
            let (shown, _) = super::widgets::input_display_text(node, style, &ty);
            shown
        } else {
            node.text_contents()
        };
        let text_w = self.font.text_width(&text, style.font_size);
        let char_w = self
            .font
            .char_width('0', style.font_size)
            .max(1.0);
        let numeric_attr = |name: &str, default: f64| {
            document::attr(node, name)
                .and_then(|v| v.trim().parse::<f64>().ok())
                .unwrap_or(default)
                .clamp(1.0, 100.0)
        };
        // 第 47 轮:行内**块容器**的固有宽 = 子树按行内累加的 max-content
        // (shrink-to-fit 的 preferred 值)。此前取 `text_contents()` 的文本宽,
        // 漏掉图标/控件等兄弟盒 —— 猎聘行业 tab(图标 20 + 间距 8 + 文本)的
        // 盒宽少 28px,子代在盒内换行溢出盒外。
        let intrinsic_floor = if is_inline_block_container(style) {
            self.inline_container_max_content(node, style).max(40.0)
        } else if tag == "select" {
            if super::widgets::select_is_list(node) {
                // 第 44 轮:列表形态 select 的固有宽 = 最长 option 文本 + 勾选位。
                let mut max_text = 0.0f64;
                if let Ok(opts) = node.select("option") {
                    for opt in opts {
                        let t = document::collapse_ws(&opt.as_node().text_contents());
                        max_text = max_text.max(self.font.text_width(&t, style.font_size));
                    }
                }
                let marker = if document::attr(node, "multiple").is_some() {
                    16.0
                } else {
                    0.0
                };
                (max_text + 6.0 + marker).max(80.0)
            } else {
                text_w + 22.0
            }
        } else if tag == "input" {
            match ty.as_str() {
                "range" => text_w.max(129.0),
                "color" => text_w.max(56.0),
                "date" | "datetime-local" | "month" | "week" => text_w.max(125.0),
                "time" => text_w.max(100.0),
                "file" => text_w.max(240.0),
                "number" => text_w.max(110.0),
                "image" => {
                    let alt = document::attr(node, "alt")
                        .filter(|v| !v.trim().is_empty())
                        .or_else(|| document::attr(node, "value"))
                        .unwrap_or_default();
                    self.font
                        .text_width(&alt, style.font_size)
                        .max(48.0)
                }
                _ => {
                    let cols = numeric_attr("size", 20.0);
                    (text_w.max(cols * char_w) + 12.0).max(40.0)
                }
            }
        } else if tag == "textarea" {
            let cols = numeric_attr("cols", 20.0);
            (text_w.max(cols * char_w) + 12.0).max(40.0)
        } else {
            text_w.max(40.0)
        };
        let w = if let Some(p) = style.width_percent {
            (p / 100.0 * avail_w).max(1.0)
        } else {
            style.width.unwrap_or(intrinsic_floor).max(1.0)
        };
        let natural_h = if tag == "textarea" {
            let rows = numeric_attr("rows", 2.0);
            (style.font_size * 1.35 * rows).max(16.0)
        } else if tag == "select" && super::widgets::select_is_list(node) {
            // 第 44 轮:列表形态 select 的固有高 = 行数 × 行高。
            let rows = super::widgets::select_list_rows(node).max(1) as f64;
            (rows * (style.font_size * 1.45).max(14.0) + 4.0).max(16.0)
        } else {
            (style.font_size * 1.45).max(16.0)
        };
        let h = style.height.unwrap_or(natural_h).max(1.0);
        (w, h)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn collect_inline(
        &mut self,
        node: &NodeRef,
        style: &ComputedStyle,
        buf: &mut InlineBuf,
        depth: usize,
        line_x: f64,
        avail_w: f64,
        inherited_tf: Option<Affine>,
        row: &mut float::InlineRow,
    ) {
        if depth > 40 || buf.chars > 200_000 {
            return;
        }
        // 行内元素自身变换(与祖先链合成):行内文本在父级 flush,变换栈里没有它,
        // 故把变换随 run 带到 flush 时应用(块级文本由 apply_group 回补)。
        let mut tf = inherited_tf;
        // 第 44 轮:行内替换元素(`img`/`svg`/`canvas`/`iframe`)是**行内原子盒**:
        // 占宽、参与断行、按基线对齐(替换元素的底边贴基线,CSS 2.1 §10.8.1)。
        let tag = document::tag_name(node).to_ascii_lowercase();
        // 第 47 轮:`<br>` 作为**块容器的直接子级**也要换行 —— 此前只有
        // `collect_inline_children`(嵌套进行内元素时)认 `<br>`,直接子级走
        // 本入口时是静默 no-op(`<div>a<br>b</div>` 的 b 紧接 a 不换行)。
        if tag == "br" {
            buf.force_break();
            return;
        }
        if super::helpers::is_replaced_tag_name(&tag) {
            let (iw, ih, _isrc) = self.replaced_size(node, style, avail_w, None);
            let atomic = InlineAtomic::new(
                node.clone(),
                style.clone(),
                AtomicKind::Replaced,
                iw,
                ih,
                line_x,
                tf,
            );
            buf.push_atomic(atomic);
            return;
        }
        // 行内元素自身建盒。此前只有"块级盒的直接行内子节点"才拿到盒子,
        // 导致 `<div><input></div>` 这类结构完全取不到几何(inspect 的 rect 恒 0×0)。
        let own_box = self.inline_box_worthy(node, style);
        let mut box_w = avail_w;
        // 子代 x 基准 = 本元素 **margin-box x**(text/img/嵌套各自由此加
        // 自身 margin+padding 得内容起点);own_box 盒需回退自身偏移。
        let mut inner_x = line_x;
        if own_box {
            let (w, h) = self.inline_box_size(node, style, avail_w);
            let (dx, dy) = relative_offset(style, avail_w, self.viewport_h);
            // 第 44 轮:表单控件走**行内原子盒**路径 —— 占宽、参与断行、
            // 按基线对齐(此前控件画在行首且把块游标提前推下去)。
            if is_form_control_tag(node) {
                let atomic = InlineAtomic::new(
                    node.clone(),
                    style.clone(),
                    AtomicKind::Control,
                    w,
                    h,
                    line_x,
                    tf,
                );
                buf.push_atomic(atomic);
                // 浮动台账:登记行占用带(供后续浮动排布),但**不**计入内容底 ——
                // 控件高度已由行盒的基线分解承担,再计一次会把块游标提前推下去。
                if w > 0.0 && h > 0.0 {
                    row.register_band(
                        line_x + style.margin_left + style.padding_left + dx,
                        self.y + dy,
                        w,
                        h,
                        line_x + avail_w,
                    );
                }
                return;
            }
            let bx = line_x + style.margin_left + style.padding_left + dx;
            let by = self.y + dy;
            // 第 47 轮:行内**块容器**(`inline-block`/`inline-flex`)走独立格式化
            // 上下文 —— 子代进独立行内缓冲、按盒内容宽换行并在盒内发射,盒高
            // 由内容决定。此前子代文本漏进父行、多行内容溢出盒外,且盒高恒为
            // 一行高(`inline_box_size` 的 `font-size × 1.45` 近似)。
            if is_inline_block_container(style) {
                // 全局游标**不**因盒高推进:盒高由 `reserve_height` 经行盒
                // 的基线分解承担(与第 44 轮行内原子盒、第 175 轮行内盒同口径),
                // 再推一次会让段首凭空下沉一个盒高(实测本例文字下沉 23px)。
                let (_content_bottom, bh) =
                    self.layout_inline_block_ctx_at(node, style, bx, by, w, tf, depth);
                row.register_band(bx, by, w, bh, line_x + avail_w);
                buf.reserve_height(bh);
                return;
            }
            // 第 47 轮:普通行内元素(`a`/`span`/`b`/`label`/…)改走**片段盒** ——
            // 收集期只把属主记在文本 run 上,盒在行提交期按片段绘制。
            // 此前 `emit_inline_box` 把盒画在**行首**且恒一行高:段落中间的
            // 链接背景整块盖到前一个词上,多行链接只有第一行有背景。
            // 空行内元素(0×0,无文本可挂片段)退回单盒路径 —— 保住第 175 轮的
            // 「空锚点仍发射寻址盒」语义,行盒仍不产生。
            let fragment_box = is_plain_inline(style) && w > 0.0 && h > 0.0;
            if !fragment_box {
                tf = self.emit_inline_box(node, style, bx, by, w, h, tf);
            } else if let Some(m) = self.own_transform(style, bx, by, w, h) {
                // 片段盒在行提交期才发射,自身变换仍须在此合成进子代变换链。
                tf = Some(match tf {
                    Some(anc) => m.compose(&anc),
                    None => m,
                });
            }
            // 阶段七:登记当前行占用,后续行内浮动可排在它的右侧剩余带。
            // 第 175 轮:空行内元素 0×0 不占流(Chrome 语义)—— 只发射寻址盒,
            // 不登记行占用、不推进游标,消灭 `<a name=x></a>` 类幽灵行高。
            if w > 0.0 && h > 0.0 {
                // 第 44 轮:行内盒**不**计入行内容底(否则段首凭空多出一个盒高的
                // 空洞,实测 `<p>Hello <a>link</a> World</p>` 文字整体下沉 23px);
                // 盒高改由行盒的 `reserve_height` 承担。
                row.register_band(bx, by, w, h, line_x + avail_w);
                buf.reserve_height(h);
            }
            box_w = w;
            inner_x = bx - style.margin_left - style.padding_left;
        }
        // 第 48 轮:`mark` 子树为空时仍画高亮底(浏览器默认 UA 背景)。
        if document::tag_name(node).eq_ignore_ascii_case("mark") && !node.children().any(|c| {
            c.as_text()
                .map(|t| !t.borrow().trim().is_empty())
                .unwrap_or_else(|| c.as_element().is_some())
        }) {
            let w = self.inline_container_max_content(node, style);
            buf.push_mark(style, inner_x, w);
            return;
        }
        // 表单控件文本/内置 UI 已由 `emit_form_control_text` 全权绘制,子文本

        // 不能再进普通行内缓冲 —— 否则 button/textarea 内容画两遍,textarea
        // 初始文本还会脱离控件盒溢出(第 42 轮)。
        // 行内块/控件盒为子代建立独立行内上下文(浮动不越出该盒),子代几何
        // 也相对自身盒求解(此前一律沿用祖先 line_x/avail_w)。
        let mut inner_row = float::InlineRow::default();
        let child_row: &mut float::InlineRow = if own_box { &mut inner_row } else { row };
        self.collect_inline_children(node, style, buf, depth, inner_x, box_w, child_row, tf);
        if own_box {
            let bottom = inner_row.content_bottom();
            if bottom > self.y {
                self.y = bottom;
            }
        }
    }

    /// `collect_inline` 的子代遍历(行内浮动入口复用同一套收集逻辑)。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn collect_inline_children(
        &mut self,
        node: &NodeRef,
        style: &ComputedStyle,
        buf: &mut InlineBuf,
        depth: usize,
        line_x: f64,
        avail_w: f64,
        row: &mut float::InlineRow,
        tf: Option<Affine>,
    ) {
        // 行内游标(第 178 轮补,第 44 轮行内原子盒之上保留):行内**原子盒**
        // (替换元素/表单控件)由行提交期的 flow_x 定位,但非原子的行内盒
        // (inline-block span 等)仍在收集期即时发射、不经行内缓冲 —— 所有
        // 兄弟共用同一 line_x 时,两个 inline-block 药丸标签(font-size:0
        // 容器内)同坐标叠印(实测猎聘行业卡「5-7年/统招本科」)。pen 按
        // margin-box 宽推进,让后续兄弟/文本落在新起点;文本段定位取
        // `flow_x.max(run.x)`,与行内流的先后关系天然协调。
        // 第 47 轮:本容器是**普通行内元素**时,其直接文本的子 run 携带
        // 片段盒属主 —— 行提交期按片段绘制背景/边框/阴影。
        let owner = self.inline_fragment_owner(node, style);
        let mut pen = line_x;
        for child in node.children() {
            if let Some(text) = child.as_text() {
                let raw = text.borrow();
                if !raw.trim().is_empty() {
                    // pen = 本元素当前游标;push_styled 再加自身
                    // margin+padding 得文本内容起点。
                    let decor = merged_text_decor(style, node);
                    let highlight = semantic_highlight(node);
                    buf.push_styled_decor(
                        &raw,
                        style,
                        pen,
                        tf,
                        style.text_shadow.clone(),
                        owner.clone(),
                        decor,
                        highlight,
                    );
                    // 文本推进游标但不越出可用右缘(同块级分支)。
                    let tw = self.font.text_width(&raw, style.font_size);
                    pen += tw.min((line_x + avail_w - pen).max(0.0));
                }
                continue;
            }
            if child.as_element().is_none() {
                continue;
            }
            let mut child_style = self.engine.computed(&child, style);
            child_style.resolve_padding_percent(if avail_w >= MAX_CONTENT_WIDTH { 0.0 } else { avail_w });
            if child_style.display == "none" {
                continue;
            }
            let tag = document::tag_name(&child);
            if tag == "br" {
                buf.force_break();
                continue;
            }
            // 第 44 轮:行内替换元素(<img>/<svg>/<canvas>/<iframe>)走**行内原子盒**
            // 路径 —— 参与行宽、断行与基线对齐(此前只抬高行高,宽度不占位、
            // 盒顶对齐行顶,且换行后仍停在段首)。
            if super::helpers::is_replaced_tag_name(&tag) {
                let (iw, ih, _isrc) = self.replaced_size(&child, &child_style, avail_w, None);
                let atomic = InlineAtomic::new(
                    child.clone(),
                    child_style.clone(),
                    AtomicKind::Replaced,
                    iw,
                    ih,
                    pen,
                    tf,
                );
                buf.push_atomic(atomic);
                // 原子盒自身经 flow_x 定位,但**后续非原子兄弟**仍读 pen,
                // 按其 margin-box 宽推进。
                pen = pen
                    + child_style.margin_left
                    + child_style.padding_left
                    + iw
                    + child_style.padding_right
                    + child_style.margin_right;
                continue;
            }
            // 阶段七:行内浮动排在当前行剩余带(输入框+按钮/图标+文字经典浮动行)。
            // 子级 margin-box 起点 = 本元素内容盒 x(margin-box + 自身 margin+padding)。
            let child_x = pen + style.margin_left + style.padding_left;
            if child_style.float != "none" {
                self.layout_inline_float(&child, &child_style, buf, depth, child_x, avail_w, row, tf);
                continue;
            }
            // 非浮动行内内容:当前行已有浮动时下沉到浮动之下(不做文字环绕)。
            if row.has_floats() {
                let y = row.float_bottom();
                if y > self.y {
                    self.y = y;
                }
                row.deactivate();
            }
            // 交互类行内元素的矩形由 `collect_inline` 入口统一建盒(元素自身),
            // 此处只负责继续收集其子树文本。inline-box 子级(含表单控件原子盒)
            // 按 margin-box 宽推进游标;纯 inline 子级按子树文本宽近似推进
            // (文本自身经 buf 流动,推进只为让**后续盒**落在文本之后;
            // 空/隐身元素推进 0,与 font-size:0 去缝容器语义一致)。
            let is_box_child = self.inline_box_worthy(&child, &child_style);
            if tag.eq_ignore_ascii_case("summary") {
                // 第 48 轮:summary 展开/折叠指示三角(浏览器 UA 语义)。
                let tri = 12.0f64.min(child_style.font_size);
                let open = document::attr(&child, "open").is_some();
                let seq = self.next_seq();
                self.display.push(DisplayItem {
                    kind: DisplayKind::WidgetGlyph {
                        glyph: WidgetGlyphKind::Caret,
                        open,
                        color: child_style.color,
                    },
                    x: child_x,
                    y: self.y,
                    w: tri,
                    h: tri,
                    radius: [CornerRadius::Px(0.0); 4],
                    z: child_style.pos.z_index.max(self.z_floor),
                    seq,
                    opacity: child_style.opacity,
                    clip: None,
                    transform: tf,
                    fixed: self.fixed_depth > 0,
                    layer: self.layer_for(&child_style),
                });
            }
            self.collect_inline(&child, &child_style, buf, depth + 1, child_x, avail_w, tf, row);
            if is_box_child {
                let (w, _h) = self.inline_box_size(&child, &child_style, avail_w);
                pen = child_x
                    + child_style.margin_left
                    + child_style.padding_left
                    + w
                    + child_style.padding_right
                    + child_style.margin_right;
            } else {
                let text = child.text_contents();
                let tw = self.font.text_width(&text, child_style.font_size);
                pen = child_x + tw.min((line_x + avail_w - child_x).max(0.0));
            }
        }
    }

    /// 行内盒统一发射(自身视觉 + 边框/阴影 + 表单控件文本),返回合成后的变换。
    pub(super) fn emit_inline_box(
        &mut self,
        node: &NodeRef,
        style: &ComputedStyle,
        bx: f64,
        by: f64,
        w: f64,
        h: f64,
        inherited_tf: Option<Affine>,
    ) -> Option<Affine> {
        let mut tf = inherited_tf;
        if let Some(m) = self.own_transform(style, bx, by, w, h) {
            tf = Some(match tf {
                Some(anc) => m.compose(&anc),
                None => m,
            });
        }
        // 阶段六:显示项发射(行内元素视觉,边框/阴影/背景/渐变全量)。
        let seq = self.next_seq();
        self.boxes.push(ElementBox {
            selector: Document::stable_selector(node),
            tag: document::tag_name(node),
            x: bx,
            y: by,
            w,
            h,
            background: style.background,
            background_alpha: style.background_alpha,
            visible: style_visible(style),
            z_index: style.pos.z_index,
            out_of_flow: false,
        });
        // 输入框/按钮等交互元素的边框与外阴影(绘制不占几何)。
        push_border_and_shadow(self, style, bx, by, w, h);
        let tag = document::tag_name(node);
        self.emit_element(seq, style, &tag, bx, by, w, h);
        self.emit_canvas_pixels(node, style, seq, bx, by, w, h);
        // 阶段七:表单控件的 value/placeholder 文本(此前只画背景+边框,
        // 输入框在截图里是没有任何提示文字的灰盒)。
        widgets::emit_form_control_text(self, node, style, &tag, bx, by, w, h, tf);
        tf
    }

    /// 行内浮动:排在当前行已占用内容的剩余带,带宽不足换行;
    /// 子代在浮动盒内按独立行内上下文排布。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn layout_inline_float(
        &mut self,
        node: &NodeRef,
        style: &ComputedStyle,
        buf: &mut InlineBuf,
        depth: usize,
        line_x: f64,
        avail_w: f64,
        row: &mut float::InlineRow,
        tf: Option<Affine>,
    ) {
        let (w, h) = self.inline_box_size(node, style, avail_w);
        let (bx, by) = row.place_float(style, line_x, line_x + avail_w, self.y, w, h);
        self.y = by;
        // 第 47 轮:浮动的行内块容器同样走独立格式化上下文(盒高由内容决定)。
        if is_inline_block_container(style) {
            let (_content_bottom, bh) =
                self.layout_inline_block_ctx_at(node, style, bx, by, w, tf, depth);
            row.note_bottom(by + bh);
            self.y = by;
            return;
        }
        let tf = self.emit_inline_box(node, style, bx, by, w, h, tf);
        if is_form_control_tag(node) {
            // 控件内容同样由 widgets 层唯一绘制,浮动路径不再收集子文本。
            row.note_bottom(by + h);
            self.y = by;
            return;
        }
        let mut inner = float::InlineRow::default();
        self.collect_inline_children(node, style, buf, depth + 1, bx, w, &mut inner, tf);
        row.note_bottom(inner.content_bottom());
        // 游标留在本行,后续浮动继续排在同一行剩余带。
        self.y = by;
    }
}
