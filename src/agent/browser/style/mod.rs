//! 样式引擎:CSS 解析、级联、继承、媒体查询与自定义属性。
//!
//! 输入是宿主侧 `Document`(短生命周期局部值),输出是纯数据 `ComputedStyle`;
//! 布局、渲染、JS 快照与 inspect 共用同一份计算样式真源。
//! 设计见 `docs/自定义浏览器和V8引擎/06-阶段二第二步CSS级联与外链样式加载设计.md`。

pub mod gradient;
pub mod selector;
pub mod transform;
pub mod values;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use kuchikiki::NodeRef;

pub use gradient::{GradientKind, GradientSpec, GradientStop};
pub use transform::{Affine, Length, Transform2D, TransformOrigin};
pub use values::BgLayer;
pub use values::BorderColor;
pub use values::BoxShadow;
pub use values::ComputedStyle;
pub use values::CornerRadius;
pub use values::FlexWrap;
pub use values::GridFlow;
pub use values::GridLine;
pub use values::TextShadow;
pub use values::TrackSize;
use values::ValueCtx;

use super::document::{self, Document};
use selector::{parse_selector_list, Selector};

// ===================== 媒体查询 =====================

#[derive(Debug, Clone, Copy)]
pub struct MediaCtx {
    pub viewport_width: f64,
    pub viewport_height: f64,
    pub prefers_dark: bool,
    pub print: bool,
}

impl Default for MediaCtx {
    fn default() -> Self {
        Self::new(1280.0, 720.0)
    }
}

impl MediaCtx {
    pub fn new(width: f64, height: f64) -> Self {
        Self {
            viewport_width: width.max(1.0),
            viewport_height: height.max(1.0),
            prefers_dark: env_on("LAEW_WEB_PREFERS_DARK"),
            print: false,
        }
    }

    fn value_ctx(&self, root_font_size: f64) -> ValueCtx {
        ValueCtx {
            viewport_width: self.viewport_width,
            viewport_height: self.viewport_height,
            root_font_size,
        }
    }
}

#[derive(Debug, Clone)]
enum MediaTerm {
    Type(String),
    Feature(String, String),
    BooleanFeature(String),
}

#[derive(Debug, Clone)]
struct MediaBranch {
    negated: bool,
    terms: Vec<MediaTerm>,
}

#[derive(Debug, Clone)]
enum MediaExpr {
    Always,
    /// 逗号分隔的 OR 分支。
    Branches(Vec<MediaBranch>),
    /// 嵌套 `@media` 的 AND。
    And(Vec<MediaExpr>),
}

impl MediaExpr {
    fn matches(&self, ctx: &MediaCtx) -> bool {
        match self {
            MediaExpr::Always => true,
            MediaExpr::Branches(branches) => branches.iter().any(|b| branch_matches(b, ctx)),
            MediaExpr::And(parts) => parts.iter().all(|p| p.matches(ctx)),
        }
    }

    fn and(self, other: MediaExpr) -> MediaExpr {
        match (self, other) {
            (MediaExpr::Always, b) => b,
            (a, MediaExpr::Always) => a,
            (a, b) => MediaExpr::And(vec![a, b]),
        }
    }
}

fn branch_matches(branch: &MediaBranch, ctx: &MediaCtx) -> bool {
    let matched = branch.terms.iter().all(|t| term_matches(t, ctx));
    if branch.negated {
        !matched
    } else {
        matched
    }
}

fn term_matches(term: &MediaTerm, ctx: &MediaCtx) -> bool {
    match term {
        MediaTerm::Type(t) => match t.as_str() {
            "all" => true,
            "screen" => !ctx.print,
            "print" => ctx.print,
            _ => false,
        },
        // 无交互态建模:触屏/悬停类特性按不匹配处理。
        MediaTerm::BooleanFeature(_) => false,
        MediaTerm::Feature(name, raw) => {
            let value = raw.trim();
            match name.as_str() {
                "min-width" => media_length(value)
                    .map(|v| ctx.viewport_width >= v)
                    .unwrap_or(false),
                "max-width" => media_length(value)
                    .map(|v| ctx.viewport_width <= v)
                    .unwrap_or(false),
                "min-height" => media_length(value)
                    .map(|v| ctx.viewport_height >= v)
                    .unwrap_or(false),
                "max-height" => media_length(value)
                    .map(|v| ctx.viewport_height <= v)
                    .unwrap_or(false),
                "orientation" => match value.to_ascii_lowercase().as_str() {
                    "landscape" => ctx.viewport_width >= ctx.viewport_height,
                    "portrait" => ctx.viewport_width < ctx.viewport_height,
                    _ => false,
                },
                "prefers-color-scheme" => match value.to_ascii_lowercase().as_str() {
                    "dark" => ctx.prefers_dark,
                    "light" => !ctx.prefers_dark,
                    _ => false,
                },
                // aspect-ratio / resolution / hover / pointer 等暂不支持。
                _ => false,
            }
        }
    }
}

fn media_length(value: &str) -> Option<f64> {
    let ctx = ValueCtx::default();
    values::parse_length(value, 16.0, &ctx)
}

fn parse_media_query(input: &str) -> MediaExpr {
    let input = input.trim();
    if input.is_empty() {
        return MediaExpr::Always;
    }
    let mut branches = Vec::new();
    for raw in split_top_level(input, ',') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let mut negated = false;
        let mut rest = raw;
        if let Some(r) = strip_word(rest, "not") {
            negated = true;
            rest = r;
        } else if let Some(r) = strip_word(rest, "only") {
            rest = r;
        }
        let mut terms = Vec::new();
        for part in split_and(rest) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some(inner) = part.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
                if let Some((name, value)) = inner.split_once(':') {
                    terms.push(MediaTerm::Feature(
                        name.trim().to_ascii_lowercase(),
                        value.trim().to_string(),
                    ));
                } else {
                    terms.push(MediaTerm::BooleanFeature(inner.trim().to_ascii_lowercase()));
                }
            } else {
                terms.push(MediaTerm::Type(part.to_ascii_lowercase()));
            }
        }
        if !terms.is_empty() {
            branches.push(MediaBranch { negated, terms });
        }
    }
    if branches.is_empty() {
        MediaExpr::Always
    } else {
        MediaExpr::Branches(branches)
    }
}

fn strip_word<'a>(input: &'a str, word: &str) -> Option<&'a str> {
    let lower = input.to_ascii_lowercase();
    if lower.starts_with(word) {
        let rest = &input[word.len()..];
        if rest
            .chars()
            .next()
            .map(|c| c.is_whitespace())
            .unwrap_or(false)
        {
            return Some(rest.trim_start());
        }
    }
    None
}

fn split_and(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth -= 1;
        }
        if depth == 0 && c.is_whitespace() {
            let rest: String = chars[i..].iter().collect();
            let trimmed = rest.trim_start();
            if trimmed
                .get(..3)
                .map(|head| head.eq_ignore_ascii_case("and"))
                .unwrap_or(false)
            {
                let after = &trimmed[3..];
                if after
                    .chars()
                    .next()
                    .map(|c| c.is_whitespace())
                    .unwrap_or(true)
                {
                    out.push(current.trim().to_string());
                    current.clear();
                    let skip_ws = chars[i..].iter().take_while(|c| c.is_whitespace()).count();
                    i += skip_ws + 3;
                    continue;
                }
            }
        }
        current.push(c);
        i += 1;
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

// ===================== 规则与样式表 =====================

#[derive(Debug, Clone)]
struct Decl {
    key: String,
    value: String,
    important: bool,
}

#[derive(Debug, Clone)]
struct Rule {
    selectors: Vec<Selector>,
    decls: Vec<Decl>,
    media: MediaExpr,
    order: usize,
}

/// 样式表概览(inspect 观测用)。
#[derive(Debug, Clone)]
pub struct SheetInfo {
    pub href: Option<String>,
    pub media: String,
    pub bytes: usize,
    pub rules: usize,
}

pub struct StyleEngine {
    rules: Vec<Rule>,
    media: MediaCtx,
    sheets: Vec<SheetInfo>,
    root_font_size: f64,
}

impl std::fmt::Debug for StyleEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StyleEngine")
            .field("rules", &self.rules.len())
            .field("sheets", &self.sheets.len())
            .field("media", &self.media)
            .finish()
    }
}

impl StyleEngine {
    /// 从文档收集 `<style>`(含宿主内联的外链样式),按文档顺序解析。
    pub fn from_document(doc: &Document, media: MediaCtx) -> Self {
        let mut rules = Vec::new();
        let mut sheets = Vec::new();
        let mut order = 0usize;
        for node in doc.select("style").unwrap_or_default() {
            let css = node.text_contents();
            let media_attr = document::attr(&node, "media").unwrap_or_default();
            let href = document::attr(&node, "data-lsm-href");
            let attr_expr = parse_media_query(&media_attr);
            let before = rules.len();
            parse_block(&css, attr_expr, &mut rules, &mut order, 0);
            sheets.push(SheetInfo {
                href,
                media: media_attr,
                bytes: css.len(),
                rules: rules.len() - before,
            });
        }
        Self {
            rules,
            media,
            sheets,
            root_font_size: 16.0,
        }
    }

    pub fn media(&self) -> MediaCtx {
        self.media
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub fn sheets(&self) -> &[SheetInfo] {
        &self.sheets
    }

    /// 文档根样式基线。
    pub fn initial_style(&self) -> ComputedStyle {
        ComputedStyle::root()
    }

    /// 计算单个元素的样式;`parent` 为其父元素的计算样式。
    pub fn computed(&self, node: &NodeRef, parent: &ComputedStyle) -> ComputedStyle {
        let tag = document::tag_name(node).to_ascii_lowercase();
        let mut st = values::ua_default(&tag, parent);
        let value_ctx = self.media.value_ctx(self.root_font_size);

        // 1. 收集命中声明(规则 + 内联)。
        let mut candidates: Vec<Candidate> = Vec::new();
        for rule in &self.rules {
            if !rule.media.matches(&self.media) {
                continue;
            }
            let mut best: Option<(u16, u16, u16)> = None;
            for sel in &rule.selectors {
                if selector::matches(sel, node) {
                    best = Some(match best {
                        Some(prev) if prev >= sel.specificity => prev,
                        _ => sel.specificity,
                    });
                }
            }
            let Some(spec) = best else { continue };
            for decl in &rule.decls {
                candidates.push(Candidate {
                    important: decl.important,
                    origin: 1,
                    spec: (0, spec.0, spec.1, spec.2),
                    order: rule.order,
                    key: decl.key.clone(),
                    value: decl.value.clone(),
                });
            }
        }
        if let Some(inline) = document::attr(node, "style") {
            for decl in parse_declarations(&inline) {
                candidates.push(Candidate {
                    important: decl.important,
                    origin: 1,
                    spec: (1, 0, 0, 0),
                    order: u32::MAX as usize,
                    key: decl.key,
                    value: decl.value,
                });
            }
        }

        // 2. 自定义属性:先继承父级,再按级联顺序覆盖。
        let mut custom = parent.custom.clone();
        let mut sorted = candidates.clone();
        sorted.sort_by_key(|c| c.sort_key());
        for c in &sorted {
            if c.key_name().starts_with("--") {
                let resolved = resolve_vars(c.value_str(), &custom, 0)
                    .unwrap_or_else(|| c.value_str().to_string());
                custom.insert(c.key_name().to_string(), resolved);
            }
        }

        // 3. 普通声明:var() 替换后按级联顺序应用。
        //    先应用 font-size,保证 em/% 长度按元素最终字号解析。
        let mut normal: Vec<(String, String)> = Vec::new();
        for c in &sorted {
            let key = c.key_name();
            if key.starts_with("--") {
                continue;
            }
            let Some(value) = resolve_vars(c.value_str(), &custom, 0) else {
                continue;
            };
            normal.push((key.to_string(), value));
        }
        for (key, value) in &normal {
            if key == "font-size" {
                values::apply_declaration(
                    &mut st,
                    key,
                    value,
                    parent,
                    &self.initial_style(),
                    &value_ctx,
                );
            }
        }
        for (key, value) in &normal {
            if key == "font-size" {
                continue;
            }
            values::apply_declaration(
                &mut st,
                key,
                value,
                parent,
                &self.initial_style(),
                &value_ctx,
            );
        }
        st.custom = custom;

        // 4. 元素级硬约束。
        if document::attr(node, "hidden").is_some()
            || document::attr(node, "aria-hidden")
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
        {
            st.display = "none".to_string();
        }
        if tag == "input"
            && document::attr(node, "type")
                .map(|v| v.eq_ignore_ascii_case("hidden"))
                .unwrap_or(false)
        {
            st.display = "none".to_string();
        }
        st
    }

    /// 从根向下计算指定元素的样式(单元素查询用)。
    pub fn computed_chain(&self, node: &NodeRef) -> ComputedStyle {
        let mut chain: Vec<NodeRef> = Vec::new();
        let mut cur = Some(node.clone());
        while let Some(n) = cur {
            if n.as_element().is_some() {
                chain.push(n.clone());
            }
            cur = n.parent();
        }
        chain.reverse();
        let mut st = self.initial_style();
        for n in chain {
            st = self.computed(&n, &st);
        }
        st
    }

    /// 元素是否真正参与渲染(自身或任一祖先 `display:none` 即整体不渲染)。
    ///
    /// 与布局层的剪枝规则(`layout/mod.rs` 里 `display:none` 跳过整棵子树)
    /// 保持一致,避免出现"元素自报可见、却没有布局盒"的矛盾状态 ——
    /// 那会让 Agent 把隐藏控件当成可交互目标。
    pub fn is_rendered(&self, node: &NodeRef) -> bool {
        let mut chain: Vec<NodeRef> = Vec::new();
        let mut cur = Some(node.clone());
        while let Some(n) = cur {
            if n.as_element().is_some() {
                chain.push(n.clone());
            }
            cur = n.parent();
        }
        chain.reverse();
        let mut st = self.initial_style();
        for n in chain {
            st = self.computed(&n, &st);
            if st.display == "none" {
                return false;
            }
        }
        true
    }
}

#[derive(Clone)]
struct Candidate {
    important: bool,
    origin: u8,
    spec: (u16, u16, u16, u16),
    order: usize,
    key: String,
    value: String,
}

impl Candidate {
    fn key_name(&self) -> &str {
        &self.key
    }

    fn value_str(&self) -> &str {
        &self.value
    }

    fn sort_key(&self) -> (u8, u8, u16, u16, u16, u16, usize) {
        (
            self.important as u8,
            self.origin,
            self.spec.0,
            self.spec.1,
            self.spec.2,
            self.spec.3,
            self.order,
        )
    }
}

// ===================== CSS 解析 =====================

fn parse_block(css: &str, media: MediaExpr, out: &mut Vec<Rule>, order: &mut usize, depth: usize) {
    if depth > 8 {
        return;
    }
    let css = strip_comments(css);
    let mut rest = css.as_str();
    while let Some((header, body, consumed)) = next_block(rest) {
        rest = &rest[consumed..];
        let header = header.trim();
        if header.is_empty() {
            continue;
        }
        if let Some(after) = header.strip_prefix('@') {
            let lower = after.to_ascii_lowercase();
            if lower.starts_with("media") {
                let query = after[5..].trim();
                let expr = parse_media_query(query);
                parse_block(&body, media.clone().and(expr), out, order, depth + 1);
            } else if lower.starts_with("supports") || lower.starts_with("layer") {
                // 乐观解析内部规则:`@supports` 常用于渐进增强,`@layer` 只影响层级顺序。
                parse_block(&body, media.clone(), out, order, depth + 1);
            }
            // @font-face / @keyframes / @import / @page / @charset 等跳过。
            continue;
        }
        let selectors = parse_selector_list(header);
        if selectors.is_empty() {
            continue;
        }
        let decls = parse_declarations(&body);
        if decls.is_empty() {
            continue;
        }
        out.push(Rule {
            selectors,
            decls,
            media: media.clone(),
            order: *order,
        });
        *order += 1;
    }
}

/// 读取下一个 `header { body }`;返回 (header, body, 已消费字节数)。
fn next_block(input: &str) -> Option<(String, String, usize)> {
    let open = input.find('{')?;
    let header = input[..open].to_string();
    let mut depth = 0i32;
    let mut end = None;
    for (idx, c) in input[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(open + idx);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    let body = input[open + 1..end].to_string();
    Some((header, body, end + 1))
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let bytes = css.as_bytes();
    let mut i = 0;
    let mut in_string: Option<char> = None;
    while i < bytes.len() {
        let c = css[i..].chars().next().unwrap_or('\0');
        if let Some(q) = in_string {
            out.push(c);
            if c == q {
                in_string = None;
            }
            i += c.len_utf8();
            continue;
        }
        if c == '"' || c == '\'' {
            in_string = Some(c);
            out.push(c);
            i += 1;
            continue;
        }
        if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            if let Some(end) = css[i + 2..].find("*/") {
                i += 2 + end + 2;
                out.push(' ');
            } else {
                break;
            }
            continue;
        }
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// 归一化 CSS 属性名:camelCase → kebab-case(`backgroundImage` → `background-image`)。
///
/// 站点(尤其百度地图)大量通过 JS `el.style.backgroundImage = ...` 写内联样式,
/// 序列化回 HTML 后属性名保留 camelCase;不归一化则 `backgroundimage` 匹配不到
/// `background-image`,背景图/对齐/层叠全部丢失。`--*` 自定义属性保持原样
/// (按 CSS 规范自定义属性区分大小写)。
fn normalize_property_key(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    if raw.starts_with("--") {
        return raw.to_ascii_lowercase();
    }
    // 厂商前缀:`Webkit*`/`Moz*`/`MS*`/`O*` → `-webkit-`/`-moz-`/`-ms-`/`-o-`
    // (CSSOM 里 `style.MSTouchAction` 对应 `-ms-touch-action`;站点常写 `MSTouchAction`)。
    for (js, css) in [
        ("Webkit", "-webkit-"),
        ("Moz", "-moz-"),
        ("MS", "-ms-"),
        ("O", "-o-"),
    ] {
        if raw.len() > js.len()
            && raw.starts_with(js)
            && raw.as_bytes()[js.len()].is_ascii_uppercase()
        {
            return format!("{}{}", css, kebab_case(&raw[js.len()..]));
        }
    }
    kebab_case(raw)
}

/// camelCase → kebab-case(`backgroundImage` → `background-image`)。
fn kebab_case(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len() + 4);
    for (i, &b) in bytes.iter().enumerate() {
        if b.is_ascii_uppercase() {
            // 前一个字符是小写字母/数字时才插入 '-'(`backgroundImage` → `background-image`;
            // 段首大写不插,`TouchAction` → `touch-action`)。
            if i > 0 && (bytes[i - 1].is_ascii_lowercase() || bytes[i - 1].is_ascii_digit()) {
                out.push('-');
            }
            out.push(b.to_ascii_lowercase() as char);
        } else {
            out.push(b as char);
        }
    }
    out
}

/// 解析声明块;支持 `!important`、括号/字符串内的 `;`/`:`。
pub fn parse_declarations(input: &str) -> Vec<Decl> {
    let mut out = Vec::new();
    for item in split_top_level(input, ';') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let Some(pos) = find_top_level_colon(item) else {
            continue;
        };
        let key = normalize_property_key(&item[..pos]);
        if key.is_empty() {
            continue;
        }
        let mut value = item[pos + 1..].trim().to_string();
        let mut important = false;
        let lower = value.to_ascii_lowercase();
        if let Some(pos) = lower.rfind("!important") {
            let tail = lower[pos + "!important".len()..].trim();
            if tail.is_empty() {
                important = true;
                value.truncate(pos);
                value = value.trim().to_string();
            }
        }
        if value.is_empty() {
            continue;
        }
        out.push(Decl {
            key,
            value,
            important,
        });
    }
    out
}

fn find_top_level_colon(input: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for (idx, c) in input.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ':' if depth == 0 => return Some(idx),
            _ => {}
        }
    }
    None
}

fn split_top_level(input: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for c in input.chars() {
        if let Some(q) = quote {
            current.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                current.push(c);
            }
            '(' | '[' => {
                depth += 1;
                current.push(c);
            }
            ')' | ']' => {
                depth -= 1;
                current.push(c);
            }
            c if c == sep && depth == 0 => {
                out.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

/// 替换 `var(--name[, fallback])`;无法解析返回 `None`。
fn resolve_vars(value: &str, custom: &BTreeMap<String, String>, depth: usize) -> Option<String> {
    if depth > 8 {
        return None;
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(pos) = rest.find("var(") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 4..];
        let end = matching_paren(after)?;
        let inner = &after[..end];
        let (name, fallback) = match split_top_level(inner, ',').as_slice() {
            [] => return None,
            [name] => (name.clone(), None),
            parts => (parts[0].clone(), Some(parts[1..].join(","))),
        };
        let name = name.trim();
        if let Some(v) = custom.get(name) {
            out.push_str(&resolve_vars(v, custom, depth + 1)?);
        } else if let Some(fb) = fallback {
            out.push_str(&resolve_vars(fb.trim(), custom, depth + 1)?);
        } else {
            return None;
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out.trim().to_string())
}

fn matching_paren(input: &str) -> Option<usize> {
    let mut depth = 1i32;
    for (idx, c) in input.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

fn env_on(key: &str) -> bool {
    matches!(
        std::env::var(key)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}
