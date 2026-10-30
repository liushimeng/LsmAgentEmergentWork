//! 行内格式化上下文:样式 run → 共享行盒的段落流(`flush_inline`)。
//!
//! 从 `layout/mod.rs` 机械拆分(单文件超 1800 行规范):`InlineBuf`/`InlineRun`
//! 为收集期缓冲模型,`Seg`/`flush_inline`/`commit_line` 为排布期行盒模型。
//! 同一视觉行可容纳多个样式段(x 连续流动、行高取段间最大值),换行点可在
//! run 边界或 run 内部;行首/行尾可折叠空白按行边裁剪。

use super::*;

impl<'a> LayoutCtx<'a> {
    /// 行内格式化上下文:把缓冲的样式 run 排成**共享行盒**的段落流。
    ///
    /// 此前每个 run 各占一行竖排(`<p>a<b>b</b>c</p>` 渲染成三行),
    /// 行内样式混合(b/span/a/颜色)一律错位 —— 现按 CSS 行内语义:
    /// 同一视觉行可容纳多个样式段,段宽连续累加,行高取段间最大值,
    /// 换行点可在 run 边界或 run 内部,行首/行尾可折叠空白按行边裁剪。
    pub(super) fn flush_inline(&mut self, buf: &mut InlineBuf, max_w: f64) {
        let max_w = max_w.max(80.0);
        // 当前视觉行:样式段 + 行高 + 段流宽度 + 下一段落点。
        let mut segs: Vec<Seg> = Vec::new();
        let mut line_h = 0.0f64;
        let mut line_w = 0.0f64;
        for run in buf.runs.drain(..) {
            // 阶段三第三步:占位 run(替换元素预留高度)只抬行高,不产出文本。
            if run.text.is_empty() {
                if segs.is_empty() {
                    // 无行在建:占位独占推进游标(收编前置 pending)。
                    self.y += self.pending_margin_bottom;
                    self.pending_margin_bottom = 0.0;
                    self.y += run.line_height;
                } else {
                    line_h = line_h.max(run.line_height);
                }
                continue;
            }
            let size = run.size.max(8.0);
            let seg_line_h = run.line_height.max(size * 1.35);
            let mut seg_text = String::new();
            let mut seg_w = 0.0f64;
            let mut seg_x = 0.0f64;
            for ch in run.text.chars() {
                if ch == '\n' {
                    if !seg_text.is_empty() {
                        segs.push(Seg {
                            text: std::mem::take(&mut seg_text),
                            x: seg_x,
                            w: seg_w,
                            run: run.clone(),
                        });
                        line_w += seg_w;
                        seg_w = 0.0;
                        line_h = line_h.max(seg_line_h);
                    }
                    self.commit_line(&mut segs, &mut line_h, line_w, max_w);
                    line_w = 0.0;
                    continue;
                }
                let w = self.font.char_width(ch, size);
                // 跨段换行判定:整行(已占宽 + 本段待入宽)超可用宽即可断,
                // 不再限定「同一 run 内」——run 边界同样可断行。
                if run.wrap && line_w + seg_w + w > max_w && (!segs.is_empty() || !seg_text.is_empty())
                {
                    if !seg_text.is_empty() {
                        segs.push(Seg {
                            text: std::mem::take(&mut seg_text),
                            x: seg_x,
                            w: seg_w,
                            run: run.clone(),
                        });
                        line_w += seg_w;
                        seg_w = 0.0;
                        line_h = line_h.max(seg_line_h);
                    }
                    self.commit_line(&mut segs, &mut line_h, line_w, max_w);
                    line_w = 0.0;
                }
                if seg_text.is_empty() {
                    // 段落点:行首取 run.x(缩进语义),行中续流取 max(流点, run.x)
                    // ——行内 margin 有余量时让出间隙,同 x 样式段连续排布。
                    seg_x = if segs.is_empty() {
                        run.x
                    } else {
                        (segs[0].x + line_w + seg_w).max(run.x)
                    };
                }
                seg_text.push(ch);
                seg_w += w;
            }
            if !seg_text.is_empty() && (run.preserve || !seg_text.trim().is_empty()) {
                segs.push(Seg {
                    text: seg_text,
                    x: seg_x,
                    w: seg_w,
                    run,
                });
                line_w += seg_w;
                line_h = line_h.max(seg_line_h);
            }
        }
        self.commit_line(&mut segs, &mut line_h, line_w, max_w);
        buf.chars = 0;
    }

    /// 提交一个视觉行:段落逐个发射文本项(共用行首 y),游标推进一次。
    /// 行内全部段落被裁剪为空(non-preserve)时整行不落地也不推进。
    fn commit_line(
        &mut self,
        segs: &mut Vec<Seg>,
        line_h: &mut f64,
        line_w: f64,
        max_w: f64,
    ) {
        if segs.is_empty() {
            *line_h = 0.0;
            return;
        }
        // 行盒落地前收编前置 pending(行盒与外边距不折叠:前块 margin_bottom
        // 在此完整落地,后块 margin_top 从行盒另起)。
        self.y += self.pending_margin_bottom;
        self.pending_margin_bottom = 0.0;
        // 同行共基线:大字号定基线,小字号下移补齐,单段行退化为旧行为。
        let max_size = segs.iter().map(|s| s.run.size).fold(0.0f64, f64::max);
        let n = segs.len();
        let mut emitted = 0usize;
        for (i, seg) in segs.iter().enumerate() {
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
                "center" => ((max_w - line_w) / 2.0).max(0.0),
                "right" => (max_w - line_w).max(0.0),
                _ => 0.0,
            };
            let line = TextLine {
                text: content,
                x: seg.x + offset,
                y: self.y + (max_size - seg.run.size).max(0.0),
                size: seg.run.size,
                color: seg.run.color,
                bold: seg.run.bold,
                visible: seg.run.visible,
            };
            // 阶段六:文本显示项(flush 时取新 seq → 与前后块级兄弟正确分层)。
            self.emit_text(&line, seg.run.transform.clone(), &seg.run.text_shadow);
            self.lines.push(line);
        }
        if emitted > 0 {
            self.y += *line_h;
        }
        segs.clear();
        *line_h = 0.0;
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

#[derive(Default)]
pub(crate) struct InlineBuf {
    runs: Vec<InlineRun>,
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
}

impl InlineBuf {
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
        if let Some(last) = self.runs.last_mut() {
            if (last.size - size).abs() < 0.1
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
        });
    }

    pub(super) fn force_break(&mut self) {
        if let Some(last) = self.runs.last_mut() {
            last.text.push('\n');
        }
    }

    /// 预留行高(阶段三第三步:行内替换元素撑开所在行)。
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
        });
    }
}

impl<'a> LayoutCtx<'a> {
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
pub(super) fn inline_box_size(&self, node: &NodeRef, style: &ComputedStyle, avail_w: f64) -> (f64, f64) {
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
        text_w + 22.0
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
        let bx = line_x + style.margin_left + style.padding_left + dx;
        let by = self.y + dy;
        tf = self.emit_inline_box(node, style, bx, by, w, h, tf);
        // 阶段七:登记当前行占用,后续行内浮动可排在它的右侧剩余带。
        // 第 175 轮:空行内元素 0×0 不占流(Chrome 语义)—— 只发射寻址盒,
        // 不登记行占用、不推进游标,消灭 `<a name=x></a>` 类幽灵行高。
        if w > 0.0 && h > 0.0 {
            row.register(bx, by, w, h, line_x + avail_w);
        }
        box_w = w;
        inner_x = bx - style.margin_left - style.padding_left;
    }
    // 表单控件文本/内置 UI 已由 `emit_form_control_text` 全权绘制,子文本
    // 不能再进普通行内缓冲 —— 否则 button/textarea 内容画两遍,textarea
    // 初始文本还会脱离控件盒溢出(第 42 轮)。
    if own_box && is_form_control_tag(node) {
        return;
    }
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
        // 阶段三第三步:行内替换元素(<img>/<svg>):记录矩形并预留行高。
        if tag.eq_ignore_ascii_case("img") || tag.eq_ignore_ascii_case("svg") {
            let (iw, ih, isrc) = self.replaced_size(&child, &child_style, avail_w, None);
            let ix = line_x + child_style.margin_left + child_style.padding_left;
            let iy = self.y;
            let visible = style_visible(&child_style);
            // 阶段六:显示项发射(元素视觉 + 内容图)。
            let seq = self.next_seq();
            self.boxes.push(ElementBox {
                selector: Document::stable_selector(&child),
                tag: "img".to_string(),
                x: ix,
                y: iy,
                w: iw,
                h: ih,
                background: child_style.background,
                background_alpha: child_style.background_alpha,
                visible,
                z_index: child_style.pos.z_index,
                out_of_flow: false,
            });
            self.emit_element(seq, &child_style, "img", ix, iy, iw, ih);
            if let Some(src) = isrc.filter(|s| self.image_loaded(s)) {
                let paint = PaintBox {
                    src,
                    x: ix,
                    y: iy,
                    w: (iw - child_style.padding_left - child_style.padding_right).max(0.0),
                    h: (ih - child_style.padding_top - child_style.padding_bottom).max(0.0),
                    fit: child_style.object_fit.clone(),
                    kind: PaintKind::Content,
                    visible,
                    z_index: child_style.pos.z_index,
                    off_x: 0.0,
                    off_y: 0.0,
                };
                let img_tf = match (self.own_transform(&child_style, ix, iy, iw, ih), tf) {
                    (Some(m), Some(anc)) => Some(m.compose(&anc)),
                    (Some(m), None) => Some(m),
                    (None, anc) => anc,
                };
                self.emit_image(&paint, child_style.opacity, img_tf);
                self.paints.push(paint);
            }
            buf.reserve_height(ih);
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
