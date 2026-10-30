//! 行内格式化上下文:样式 run → 共享行盒的段落流(`flush_inline`)。
//!
//! 从 `layout/mod.rs` 机械拆分(单文件超 1800 行规范):`InlineBuf`/`InlineRun`
//! 为收集期缓冲模型,`Seg`/`flush_inline`/`commit_line` 为排布期行盒模型。
//! 同一视觉行可容纳多个样式段(x 连续流动、行高取段间最大值),换行点可在
//! run 边界或 run 内部;行首/行尾可折叠空白按行边裁剪。
//!
//! 第 44 轮:引入**行内原子盒**(替换元素与表单控件)—— 它们参与行宽、
//! 断行与基线对齐,详见 `inline_atomic.rs` 的模块文档。

use super::inline_atomic::{AtomicKind, InlineAtomic, VAlign};
use super::*;

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
    /// 收集顺序(与原子盒共用序号)。
    order: u64,
    /// 已「封口」:其后不允许再并入同风格文本(原子盒前后不得合并,
    /// 否则会丢掉原子盒在行内的先后位置)。
    sealed: bool,
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
    ) {
        self.chars += text.chars().count();
        let size = st.font_size;
        let bold = st.font_weight >= 600;
        let visible = st.visibility != "hidden" && st.visibility != "collapse" && st.opacity > 0.0;
        let x = x_base + st.margin_left + st.padding_left;
        let preserve = matches!(st.white_space.as_str(), "pre" | "pre-wrap" | "break-spaces");
        let wrap = !matches!(st.white_space.as_str(), "pre" | "nowrap");
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
            {
                last.text.push_str(text);
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
            order,
            sealed: false,
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
        }
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
            order,
            sealed: false,
        });
    }
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
        let max_w = max_w.max(80.0);
        let items: &InlineBuf = buf;
        let mut line = LineState::new();
        let mut ri = 0usize;
        let mut ai = 0usize;
        while ri < items.runs.len() || ai < items.atomics.len() {
            let run_first = ai >= items.atomics.len()
                || (ri < items.runs.len() && items.runs[ri].order < items.atomics[ai].order);
            if run_first {
                let run = items.runs[ri].clone();
                ri += 1;
                self.flush_run(&run, items, &mut line, max_w);
            } else {
                let idx = ai;
                ai += 1;
                self.flush_atomic(idx, items, &mut line, max_w);
            }
        }
        self.commit_line(&mut line, max_w, items);
        // 缓冲已排空:文本 run 与原子盒都消费掉(否则下一次 flush 会重复排布)。
        buf.runs.clear();
        buf.atomics.clear();
        buf.chars = 0;
    }

    /// 把一个文本 run 排入当前行(必要时断行)。
    fn flush_run(
        &mut self,
        run: &InlineRun,
        items: &InlineBuf,
        line: &mut LineState,
        max_w: f64,
    ) {
        // 空文本占位 run:只用于预留行高,不产出文本。
        if run.text.is_empty() {
            line.ascent = line.ascent.max(run.size);
            line.descent = line.descent.max((run.line_height - run.size).max(0.0));
            return;
        }
        let size = run.size.max(8.0);
        let seg_line_h = run.line_height.max(size * 1.35);
        let mut seg_text = String::new();
        let mut seg_w = 0.0f64;
        let mut seg_x = 0.0f64;
        for ch in run.text.chars() {
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
            }
            if seg_text.is_empty() {
                // 段落点:行首取 run.x(缩进语义),行中续流取 max(流点, run.x)
                // ——行内 margin 有余量时让出间隙,同 x 样式段连续排布。
                if !line.started {
                    line.origin_x = run.x;
                    line.flow_x = run.x;
                    line.started = true;
                }
                seg_x = line.flow_x.max(run.x);
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
            if content.is_empty() {
                continue;
            }
            emitted += 1;
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
            };
            // 阶段六:文本显示项(flush 时取新 seq → 与前后块级兄弟正确分层)。
            self.emit_text(&text_line, seg.run.transform.clone(), &seg.run.text_shadow);
            self.lines.push(text_line);
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
        // 第 41 轮:带内置 UI 的 input 类型给类型化固有宽(浏览器量级),
        // 免得滑轨/色块/日历图标被挤进 40px 的窄盒里看不出是控件。
        // 第 42 轮:文本类再并入 value/placeholder 与 `size` 列宽。
        let intrinsic_floor = if tag == "select" {
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
        if matches!(tag.as_str(), "img" | "svg" | "canvas" | "iframe") {
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
            tf = self.emit_inline_box(node, style, bx, by, w, h, tf);
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
        for child in node.children() {
            if let Some(text) = child.as_text() {
                let raw = text.borrow();
                if !raw.trim().is_empty() {
                    // line_x = 本元素 margin-box x;push_styled 再加自身
                    // margin+padding 得文本内容起点。
                    buf.push_styled(&raw, style, line_x, tf, style.text_shadow.clone());
                }
                continue;
            }
            if child.as_element().is_none() {
                continue;
            }
            let child_style = self.engine.computed(&child, style);
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
            if matches!(
                tag.to_ascii_lowercase().as_str(),
                "img" | "svg" | "canvas" | "iframe"
            ) {
                let (iw, ih, _isrc) = self.replaced_size(&child, &child_style, avail_w, None);
                let atomic = InlineAtomic::new(
                    child.clone(),
                    child_style.clone(),
                    AtomicKind::Replaced,
                    iw,
                    ih,
                    line_x,
                    tf,
                );
                buf.push_atomic(atomic);
                continue;
            }
            // 阶段七:行内浮动排在当前行剩余带(输入框+按钮/图标+文字经典浮动行)。
            // 子级 margin-box 起点 = 本元素内容盒 x(margin-box + 自身 margin+padding)。
            let child_x = line_x + style.margin_left + style.padding_left;
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
            // 此处只负责继续收集其子树文本。
            self.collect_inline(&child, &child_style, buf, depth + 1, child_x, avail_w, tf, row);
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
