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

