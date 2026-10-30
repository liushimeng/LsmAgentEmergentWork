//! CSS 选择器:解析、匹配与特异性。
//!
//! 支持后代 / 子代 `>` / 相邻兄弟 `+` / 一般兄弟 `~` 四种组合符,
//! 复合选择器(标签、`*`、`#id`、`.class`、属性、伪类)。
//! 伪元素规则由上层跳过;未知伪类恒不匹配。
//!
//! 第 41 轮:`:hover`/`:focus`/`:focus-visible`/`:active` 由**运行时伪类状态**
//! 驱动 —— 状态经 `Document::apply_widget_state` 以 `data-lsm-*` 属性写入
//! 临时解析树,匹配时按属性查表(与 `:checked`/`:disabled` 同构)。

use kuchikiki::NodeRef;

use super::super::document;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    Descendant,
    Child,
    Adjacent,
    GeneralSibling,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttrOp {
    Exists,
    Equals,
    Includes,
    Prefix,
    Suffix,
    Substring,
    DashMatch,
}

#[derive(Debug, Clone)]
pub struct AttrSelector {
    pub name: String,
    pub op: AttrOp,
    pub value: String,
}

#[derive(Debug, Clone)]
pub enum Pseudo {
    FirstChild,
    LastChild,
    OnlyChild,
    FirstOfType,
    LastOfType,
    OnlyOfType,
    NthChild(i32, i32),
    NthOfType(i32, i32),
    NthLastChild(i32, i32),
    NthLastOfType(i32, i32),
    Not(Box<Compound>),
    Root,
    Empty,
    Checked,
    /// 第 42 轮:checkbox/progress 的半选/不确定态(由 `indeterminate` 属性承载)。
    Indeterminate,
    Disabled,
    Enabled,
    Link,
    /// 第 41 轮:运行时交互态伪类(由 `data-lsm-*` 属性标记驱动)。
    Hover,
    Focus,
    FocusVisible,
    Active,
    /// 动态伪类/未知伪类:恒不匹配。
    Never,
}

#[derive(Debug, Clone, Default)]
pub struct Compound {
    pub tag: Option<String>,
    pub ids: Vec<String>,
    pub classes: Vec<String>,
    pub attrs: Vec<AttrSelector>,
    pub pseudos: Vec<Pseudo>,
}

#[derive(Debug, Clone)]
pub struct Selector {
    /// `parts[i].0` 是 `parts[i-1]` 与 `parts[i]` 之间的组合符;首项占位为后代。
    pub parts: Vec<(Combinator, Compound)>,
    /// `(id, class/attr/pseudo-class, tag)`。
    pub specificity: (u16, u16, u16),
}

/// 解析逗号分隔的选择器组;非法选择器被丢弃(不影响同组其它选择器)。
pub fn parse_selector_list(input: &str) -> Vec<Selector> {
    split_top_level(input, ',')
        .into_iter()
        .filter_map(|s| parse_selector(s.trim()))
        .collect()
}

/// 解析单个选择器。返回 `None` 表示非法或含伪元素(不参与匹配)。
pub fn parse_selector(input: &str) -> Option<Selector> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let raw_parts = split_compounds(input)?;
    if raw_parts.is_empty() {
        return None;
    }
    let mut parts = Vec::with_capacity(raw_parts.len());
    let mut spec = (0u16, 0u16, 0u16);
    for (comb, raw) in raw_parts {
        let compound = parse_compound(&raw)?;
        let (a, b, c) = compound_specificity(&compound);
        spec.0 = spec.0.saturating_add(a);
        spec.1 = spec.1.saturating_add(b);
        spec.2 = spec.2.saturating_add(c);
        parts.push((comb, compound));
    }
    Some(Selector {
        parts,
        specificity: spec,
    })
}

/// 匹配节点。仅元素节点可命中。
pub fn matches(selector: &Selector, node: &NodeRef) -> bool {
    if node.as_element().is_none() || selector.parts.is_empty() {
        return false;
    }
    let last = selector.parts.len() - 1;
    if !match_compound(node, &selector.parts[last].1) {
        return false;
    }
    match_from(node, selector, last)
}

fn match_from(node: &NodeRef, selector: &Selector, idx: usize) -> bool {
    if idx == 0 {
        return true;
    }
    let target = &selector.parts[idx - 1].1;
    match selector.parts[idx].0 {
        Combinator::Descendant => {
            let mut cur = node.parent();
            while let Some(n) = cur {
                if n.as_element().is_some()
                    && match_compound(&n, target)
                    && match_from(&n, selector, idx - 1)
                {
                    return true;
                }
                cur = n.parent();
            }
            false
        }
        Combinator::Child => match node.parent() {
            Some(parent) => {
                parent.as_element().is_some()
                    && match_compound(&parent, target)
                    && match_from(&parent, selector, idx - 1)
            }
            None => false,
        },
        Combinator::Adjacent => match previous_element_sibling(node) {
            Some(prev) => match_compound(&prev, target) && match_from(&prev, selector, idx - 1),
            None => false,
        },
        Combinator::GeneralSibling => {
            let mut cur = previous_element_sibling(node);
            while let Some(prev) = cur {
                if match_compound(&prev, target) && match_from(&prev, selector, idx - 1) {
                    return true;
                }
                cur = previous_element_sibling(&prev);
            }
            false
        }
    }
}

// ===================== 解析 =====================

/// 按顶层组合符切分为 `(前导组合符, 复合选择器文本)`。
fn split_compounds(input: &str) -> Option<Vec<(Combinator, String)>> {
    let mut out: Vec<(Combinator, String)> = Vec::new();
    let mut current = String::new();
    let mut pending: Option<Combinator> = None;
    let mut depth = 0i32;
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '(' | '[' => {
                depth += 1;
                current.push(c);
            }
            ')' | ']' => {
                depth -= 1;
                current.push(c);
            }
            '>' | '+' | '~' if depth == 0 => {
                if !current.trim().is_empty() {
                    out.push((
                        pending.take().unwrap_or(Combinator::Descendant),
                        current.trim().to_string(),
                    ));
                    current.clear();
                }
                pending = Some(match c {
                    '>' => Combinator::Child,
                    '+' => Combinator::Adjacent,
                    _ => Combinator::GeneralSibling,
                });
            }
            c if c.is_whitespace() && depth == 0 => {
                if !current.trim().is_empty() {
                    out.push((
                        pending.take().unwrap_or(Combinator::Descendant),
                        current.trim().to_string(),
                    ));
                    current.clear();
                }
            }
            _ => current.push(c),
        }
        i += 1;
    }
    if !current.trim().is_empty() {
        out.push((
            pending.take().unwrap_or(Combinator::Descendant),
            current.trim().to_string(),
        ));
    }
    if pending.is_some() {
        // 以组合符结尾(如 `div >`):非法。
        return None;
    }
    Some(out)
}

fn parse_compound(input: &str) -> Option<Compound> {
    let mut out = Compound::default();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    // 裸 `*`(通配)合法:compound 全空 = 匹配任意元素;特异性 (0,0,0)。
    let mut universal = false;
    // 标签或 `*`
    if i < chars.len() && (chars[i].is_ascii_alphabetic() || chars[i] == '*') {
        let start = i;
        i += 1;
        while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '-') {
            i += 1;
        }
        let tag: String = chars[start..i].iter().collect();
        if tag != "*" {
            out.tag = Some(tag.to_ascii_lowercase());
        } else {
            universal = true;
        }
    }
    while i < chars.len() {
        match chars[i] {
            '#' => {
                i += 1;
                let start = i;
                while i < chars.len() && is_ident_char(chars[i]) {
                    i += 1;
                }
                if start == i {
                    return None;
                }
                out.ids.push(chars[start..i].iter().collect());
            }
            '.' => {
                i += 1;
                let start = i;
                while i < chars.len() && is_ident_char(chars[i]) {
                    i += 1;
                }
                if start == i {
                    return None;
                }
                out.classes.push(chars[start..i].iter().collect());
            }
            '[' => {
                let (attr, next) = parse_attr(&chars, i)?;
                out.attrs.push(attr);
                i = next;
            }
            ':' => {
                if i + 1 < chars.len() && chars[i + 1] == ':' {
                    // 伪元素:整条选择器不参与匹配。
                    return None;
                }
                i += 1;
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '-') {
                    i += 1;
                }
                if start == i {
                    return None;
                }
                let name: String = chars[start..i]
                    .iter()
                    .collect::<String>()
                    .to_ascii_lowercase();
                let arg = if i < chars.len() && chars[i] == '(' {
                    let mut depth = 1;
                    i += 1;
                    let start = i;
                    while i < chars.len() && depth > 0 {
                        match chars[i] {
                            '(' => depth += 1,
                            ')' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                    if i >= chars.len() {
                        return None;
                    }
                    let arg: String = chars[start..i].iter().collect();
                    i += 1; // 跳过 ')'
                    Some(arg)
                } else {
                    None
                };
                out.pseudos.push(parse_pseudo(&name, arg.as_deref())?);
            }
            _ => return None,
        }
    }
    if !universal
        && out.tag.is_none()
        && out.ids.is_empty()
        && out.classes.is_empty()
        && out.attrs.is_empty()
        && out.pseudos.is_empty()
    {
        return None;
    }
    Some(out)
}

fn parse_attr(chars: &[char], start: usize) -> Option<(AttrSelector, usize)> {
    let mut i = start + 1;
    let mut depth = 0;
    while i < chars.len() {
        match chars[i] {
            '[' => depth += 1,
            ']' if depth == 0 => break,
            ']' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    if i >= chars.len() {
        return None;
    }
    let expr: String = chars[start + 1..i].iter().collect();
    let next = i + 1;
    let expr = expr.trim();
    if expr.is_empty() {
        return None;
    }
    // 支持 = ~= ^= $= *= |=
    let ops = ["~=", "^=", "$=", "*=", "|=", "="];
    for op in ops {
        if let Some(pos) = find_top_level_op(expr, op) {
            let name = expr[..pos].trim().to_ascii_lowercase();
            let value = expr[pos + op.len()..]
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_string();
            if name.is_empty() {
                return None;
            }
            let op = match op {
                "~=" => AttrOp::Includes,
                "^=" => AttrOp::Prefix,
                "$=" => AttrOp::Suffix,
                "*=" => AttrOp::Substring,
                "|=" => AttrOp::DashMatch,
                _ => AttrOp::Equals,
            };
            return Some((AttrSelector { name, op, value }, next));
        }
    }
    Some((
        AttrSelector {
            name: expr.to_ascii_lowercase(),
            op: AttrOp::Exists,
            value: String::new(),
        },
        next,
    ))
}

fn find_top_level_op(expr: &str, op: &str) -> Option<usize> {
    // 属性表达式内无嵌套,直接查找;`=` 需避免命中 `~=` 等的尾字符。
    let bytes = expr.as_bytes();
    let mut idx = 0;
    while let Some(pos) = expr[idx..].find(op) {
        let p = idx + pos;
        if op == "=" && p > 0 {
            let prev = bytes[p - 1] as char;
            if matches!(prev, '~' | '^' | '$' | '*' | '|') {
                idx = p + 1;
                continue;
            }
        }
        return Some(p);
    }
    None
}

fn parse_pseudo(name: &str, arg: Option<&str>) -> Option<Pseudo> {
    Some(match name {
        "first-child" => Pseudo::FirstChild,
        "last-child" => Pseudo::LastChild,
        "only-child" => Pseudo::OnlyChild,
        "first-of-type" => Pseudo::FirstOfType,
        "last-of-type" => Pseudo::LastOfType,
        "only-of-type" => Pseudo::OnlyOfType,
        "nth-child" => {
            let (a, b) = parse_nth(arg?)?;
            Pseudo::NthChild(a, b)
        }
        "nth-of-type" => {
            let (a, b) = parse_nth(arg?)?;
            Pseudo::NthOfType(a, b)
        }
        "nth-last-child" => {
            let (a, b) = parse_nth(arg?)?;
            Pseudo::NthLastChild(a, b)
        }
        "nth-last-of-type" => {
            let (a, b) = parse_nth(arg?)?;
            Pseudo::NthLastOfType(a, b)
        }
        "not" => Pseudo::Not(Box::new(parse_compound(arg?.trim())?)),
        "root" => Pseudo::Root,
        "empty" => Pseudo::Empty,
        "checked" => Pseudo::Checked,
        "indeterminate" => Pseudo::Indeterminate,
        "disabled" => Pseudo::Disabled,
        "enabled" => Pseudo::Enabled,
        "link" | "any-link" => Pseudo::Link,
        "hover" => Pseudo::Hover,
        "focus" => Pseudo::Focus,
        "focus-visible" => Pseudo::FocusVisible,
        "active" => Pseudo::Active,
        _ => Pseudo::Never,
    })
}

/// 解析 `an+b`;`odd` = `2n+1`,`even` = `2n`。
pub fn parse_nth(arg: &str) -> Option<(i32, i32)> {
    let s = arg.trim().to_ascii_lowercase().replace(' ', "");
    match s.as_str() {
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        _ => {}
    }
    if let Ok(n) = s.parse::<i32>() {
        return Some((0, n));
    }
    let (a_part, b_part) = match s.split_once('n') {
        Some((a, b)) => (a, b),
        None => return None,
    };
    let a = match a_part {
        "" | "+" => 1,
        "-" => -1,
        _ => a_part.parse::<i32>().ok()?,
    };
    let b = match b_part {
        "" => 0,
        _ => b_part.trim_start_matches('+').parse::<i32>().ok()?,
    };
    Some((a, b))
}

/// 选择器的特异性 `(id, class/attr/伪类, 类型)`。
///
/// 供测试与诊断使用(第 41 轮:伪类必须计入伪类列)。
pub fn selector_specificity(input: &str) -> (u16, u16, u16) {
    parse_selector(input).map(|s| s.specificity).unwrap_or((0, 0, 0))
}

fn compound_specificity(c: &Compound) -> (u16, u16, u16) {
    let mut a = c.ids.len() as u16;
    let mut b = (c.classes.len() + c.attrs.len()) as u16;
    let mut cc: u16 = if c.tag.is_some() { 1 } else { 0 };
    for p in &c.pseudos {
        match p {
            Pseudo::Not(inner) => {
                let (ia, ib, ic) = compound_specificity(inner);
                a = a.saturating_add(ia);
                b = b.saturating_add(ib);
                cc = cc.saturating_add(ic);
            }
            Pseudo::Never => {}
            _ => b = b.saturating_add(1),
        }
    }
    (a, b, cc)
}

// ===================== 匹配 =====================

fn match_compound(node: &NodeRef, compound: &Compound) -> bool {
    let Some(el) = node.as_element() else {
        return false;
    };
    let tag = el.name.local.to_string().to_ascii_lowercase();
    if let Some(expected) = &compound.tag {
        if &tag != expected {
            return false;
        }
    }
    let attrs = el.attributes.borrow();
    if !compound.ids.is_empty() {
        let Some(id) = attrs.get("id") else {
            return false;
        };
        if !compound.ids.iter().all(|want| want == id) {
            return false;
        }
    }
    if !compound.classes.is_empty() {
        let class_attr = attrs.get("class").map(str::to_string).unwrap_or_default();
        let classes: Vec<&str> = class_attr.split_whitespace().collect();
        if !compound
            .classes
            .iter()
            .all(|want| classes.contains(&want.as_str()))
        {
            return false;
        }
    }
    for attr in &compound.attrs {
        let Some(actual) = attrs.get(attr.name.as_str()) else {
            return false;
        };
        if !attr_matches(actual, attr) {
            return false;
        }
    }
    drop(attrs);
    for pseudo in &compound.pseudos {
        if !pseudo_matches(node, &tag, pseudo) {
            return false;
        }
    }
    true
}

fn attr_matches(actual: &str, attr: &AttrSelector) -> bool {
    match attr.op {
        AttrOp::Exists => true,
        AttrOp::Equals => actual == attr.value,
        AttrOp::Includes => actual.split_whitespace().any(|v| v == attr.value),
        AttrOp::Prefix => actual.starts_with(&attr.value),
        AttrOp::Suffix => actual.ends_with(&attr.value),
        AttrOp::Substring => actual.contains(&attr.value),
        AttrOp::DashMatch => {
            actual == attr.value || actual.starts_with(&format!("{}-", attr.value))
        }
    }
}

fn pseudo_matches(node: &NodeRef, tag: &str, pseudo: &Pseudo) -> bool {
    match pseudo {
        Pseudo::FirstChild => element_index(node) == 1,
        Pseudo::LastChild => element_index(node) == element_count(node),
        Pseudo::OnlyChild => element_count(node) == 1,
        Pseudo::FirstOfType => type_index(node, tag) == 1,
        Pseudo::LastOfType => type_index(node, tag) == type_count(node, tag),
        Pseudo::OnlyOfType => type_count(node, tag) == 1,
        Pseudo::NthChild(a, b) => nth_matches(element_index(node) as i32, *a, *b),
        Pseudo::NthOfType(a, b) => nth_matches(type_index(node, tag) as i32, *a, *b),
        Pseudo::NthLastChild(a, b) => nth_matches(
            (element_count(node) as i32) - (element_index(node) as i32) + 1,
            *a,
            *b,
        ),
        Pseudo::NthLastOfType(a, b) => {
            let total = type_count(node, tag);
            nth_matches((total as i32) - (type_index(node, tag) as i32) + 1, *a, *b)
        }
        Pseudo::Not(inner) => !match_compound(node, inner),
        Pseudo::Root => tag == "html",
        Pseudo::Empty => node.children().all(|c| {
            c.as_element().is_none()
                && c.as_text()
                    .map(|t| t.borrow().trim().is_empty())
                    .unwrap_or(true)
        }),
        Pseudo::Checked => {
            let Some(el) = node.as_element() else {
                return false;
            };
            let attrs = el.attributes.borrow();
            attrs.contains("checked") || attrs.contains("selected")
        }
        // 与 JS 选择器桥同口径:内核以 `indeterminate` 属性承载半选态,
        // `Document::apply_widget_state` 与 `el.indeterminate = true` 都会写它。
        Pseudo::Indeterminate => {
            matches!(tag, "input" | "progress")
                && node
                    .as_element()
                    .map(|e| e.attributes.borrow().contains("indeterminate"))
                    .unwrap_or(false)
        }
        Pseudo::Disabled => {
            let Some(el) = node.as_element() else {
                return false;
            };
            el.attributes.borrow().contains("disabled")
        }
        Pseudo::Enabled => {
            let Some(el) = node.as_element() else {
                return false;
            };
            matches!(
                tag,
                "input" | "button" | "select" | "textarea" | "option" | "fieldset"
            ) && !el.attributes.borrow().contains("disabled")
        }
        Pseudo::Link => {
            matches!(tag, "a" | "area")
                && node
                    .as_element()
                    .map(|e| e.attributes.borrow().contains("href"))
                    .unwrap_or(false)
        }
        // 第 41 轮:运行时交互态。状态经 `Document::apply_widget_state` 以
        // `data-lsm-*` 属性写入临时解析树,匹配时按属性查表 —— 与
        // `:checked`/`:disabled` 同一套「运行时覆盖 → 属性 → 样式」通路。
        Pseudo::Hover => has_state_attr(node, document::ATTR_PSEUDO_HOVER),
        Pseudo::Focus => has_state_attr(node, document::ATTR_PSEUDO_FOCUS),
        Pseudo::FocusVisible => has_state_attr(node, document::ATTR_PSEUDO_FOCUS_VISIBLE),
        Pseudo::Active => has_state_attr(node, document::ATTR_PSEUDO_ACTIVE),
        Pseudo::Never => false,
    }
}

/// 运行时伪类状态标记属性是否存在。
fn has_state_attr(node: &NodeRef, name: &str) -> bool {
    node.as_element()
        .map(|e| e.attributes.borrow().contains(name))
        .unwrap_or(false)
}

fn nth_matches(index: i32, a: i32, b: i32) -> bool {
    if index < 1 {
        return false;
    }
    if a == 0 {
        return index == b;
    }
    let diff = index - b;
    diff % a == 0 && diff / a >= 0
}

fn element_index(node: &NodeRef) -> usize {
    let Some(parent) = node.parent() else {
        return 1;
    };
    let mut idx = 0;
    for child in parent.children() {
        if child.as_element().is_some() {
            idx += 1;
            if child == *node {
                return idx;
            }
        }
    }
    idx.max(1)
}

fn element_count(node: &NodeRef) -> usize {
    node.parent()
        .map(|p| p.children().filter(|c| c.as_element().is_some()).count())
        .unwrap_or(1)
        .max(1)
}

fn type_index(node: &NodeRef, tag: &str) -> usize {
    let Some(parent) = node.parent() else {
        return 1;
    };
    let mut idx = 0;
    for child in parent.children() {
        if let Some(el) = child.as_element() {
            if el.name.local.to_string().eq_ignore_ascii_case(tag) {
                idx += 1;
                if child == *node {
                    return idx;
                }
            }
        }
    }
    idx.max(1)
}

fn type_count(node: &NodeRef, tag: &str) -> usize {
    node.parent()
        .map(|p| {
            p.children()
                .filter(|c| {
                    c.as_element()
                        .map(|e| e.name.local.to_string().eq_ignore_ascii_case(tag))
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(1)
        .max(1)
}

fn previous_element_sibling(node: &NodeRef) -> Option<NodeRef> {
    let parent = node.parent()?;
    let mut prev: Option<NodeRef> = None;
    for child in parent.children() {
        if child == *node {
            return prev;
        }
        if child.as_element().is_some() {
            prev = Some(child);
        }
    }
    None
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
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
