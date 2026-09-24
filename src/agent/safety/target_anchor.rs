//! 任务锚点(TargetAnchor)与防目标漂移(第 128 轮,2026-09-24)。
//!
//! ## 要解决的问题
//!
//! 实测(日志 `llaew_20260924_151442.log`):用户提示词含 `https://www.anthropic.com/`,
//! 但多行粘贴被截断后只剩「打开网站搜索,这个网站最新时间的 3 个文章…」——
//! Agent 先猜 `news.ycombinator.com`(超时),再**自行改派** `ithome.com`,
//! 在错误站点上完整交付并被判「✅ 成功」。用户要求:**「可以失败,但是不能乱跑」**。
//!
//! ## 设计要点
//!
//! 1. **机械抽取优先于 LLM 判断**:锚点由正则从用户原文抽取,不经模型 ——
//!    因此不会被摘要压缩掉、不会被幻觉改写、不会在跨 Agent 传递中漂移。
//! 2. **宁可失败,不可替换**:所有越界路径的出口都是「结构化失败 + 明确原因」,
//!    而不是「换一个试试」。
//! 3. **只在确定可判定时强制**:锚点为空(用户没给站点)时不阻断任何导航,
//!    避免误伤「搜一下最新 Rust 新闻」这类合法开放任务。
//! 4. **fail-open**:抽取/解析出错一律退回「无锚点」(不阻断),
//!    防漂移机制自身绝不能成为新的失败面。
//!
//! ## 四层防御中的位置
//!
//! 本模块是 L1(澄清门)/ L3(工具门)/ L4(验收门)共用的**唯一事实源**:
//! - L1 `unresolved_reference` → 编排器澄清短路(回问用户,不进 WorkFlow);
//! - L3 `check_open_against_target_anchor` → `MCP_Web_Use action=open` 越界返回 code=6001;
//! - L4 `detect_target_drift` → `ExecutionTrace` 的 `target_drift` 失败信号 + QC 目标一致性门。
//!
//! 设计见 `tmpPlan/2026-09-24_03-任务锚点与防目标漂移根治方案.md`。

use std::sync::{Mutex, OnceLock};

use once_cell::sync::Lazy;
use regex::Regex;

use crate::llm::{ChatMessage, Role};

/// 任务锚点全关开关:`LAEW_TARGET_ANCHOR=off|0|false|no`。
///
/// 关闭时 `current_target_anchor()` 恒返回 `None`,L1/L3/L4 三层同时归零,
/// 严格回退到第 127 轮行为(与 `LAEW_MCP_ENABLED` / `LAEW_SELF_SPAWN` 同构)。
pub fn target_anchor_enabled() -> bool {
    !matches!(
        std::env::var("LAEW_TARGET_ANCHOR").ok().as_deref(),
        Some("off") | Some("0") | Some("false") | Some("no")
    )
}

/// L3 工具级**阻断**开关:`LAEW_TARGET_ANCHOR_BLOCK=off|0|false|no`(观察模式)。
///
/// 关闭时 `check_open_against_target_anchor` 恒返回 `None`(不返回 6001),
/// 但 L1 澄清门 / L4 信号与 QC 门仍然生效 —— 用于灰度期只观察不拦截。
pub fn target_anchor_block_enabled() -> bool {
    !matches!(
        std::env::var("LAEW_TARGET_ANCHOR_BLOCK").ok().as_deref(),
        Some("off") | Some("0") | Some("false") | Some("no")
    )
}

/// 常见多段公共后缀(不引入 publicsuffix crate,遵守「不引入新 crate」约定)。
///
/// 只收录会让「取末两段」出错的高频后缀;未收录的退化为末两段启发式,
/// 对锚点比对而言是**保守方向**(归一后的域更长 → 匹配更严 → 不会误放行)。
const MULTI_PART_PUBLIC_SUFFIXES: &[&str] = &[
    "com.cn", "net.cn", "org.cn", "gov.cn", "edu.cn", "ac.cn", "co.uk", "org.uk", "ac.uk",
    "gov.uk", "co.jp", "or.jp", "ne.jp", "ac.jp", "go.jp", "com.au", "net.au", "org.au",
    "gov.au", "edu.au", "co.kr", "or.kr", "go.kr", "com.tw", "org.tw", "gov.tw", "edu.tw",
    "com.hk", "org.hk", "gov.hk", "com.sg", "com.my", "co.in", "net.in", "org.in",
    "gov.in", "co.za", "com.br", "net.br", "org.br", "gov.br", "com.mx", "com.ar",
    "co.nz", "net.nz", "org.nz", "govt.nz", "com.tr", "org.tr", "gov.tr", "com.ru",
    "co.il", "org.il", "gov.il", "com.ua", "com.pl", "co.id", "com.ph", "com.vn",
    "co.th", "com.pe", "com.co", "com.ve", "com.ec", "com.eg", "com.sa", "com.pk",
];

/// 裸主机(无 scheme)识别用的常见 TLD 白名单。
///
/// 只在 TLD 命中本表时才把「a.b」形态当成主机,避免把 `mod.rs` / `main.rs` /
/// `Cargo.toml` 这类**文件路径**误判成域名(实测 SubAgent 提示词里满是源码路径)。
const KNOWN_TLDS: &[&str] = &[
    "com", "cn", "net", "org", "io", "ai", "dev", "app", "me", "co", "info", "biz", "xyz",
    "tech", "online", "site", "cloud", "gov", "edu", "mil", "int", "ru", "jp", "uk", "de",
    "fr", "kr", "in", "br", "au", "ca", "it", "nl", "se", "no", "fi", "dk", "pl", "cz",
    "at", "ch", "be", "es", "pt", "gr", "tr", "za", "mx", "ar", "cl", "sg", "hk", "tw",
    "th", "vn", "my", "id", "ph", "nz", "ie", "il", "ae", "sa", "ua", "eu", "asia", "pro",
    "name", "mobi", "tel", "travel", "sh", "gg", "to", "tv", "cc", "ws", "ly", "fm", "am",
];

/// `http(s)://…` 完整 URL。终止字符集排除空白、引号、反引号、括号与中文标点
/// (用户提示词常写成 `` `https://x.com/` `` 或「地址 https://x.com ，然后…」)。
static RE_SCHEME_URL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)\bhttps?://[^\s<>"'`\)\]\}，。、;；,]+"#).expect("RE_SCHEME_URL 静态正则合法")
});

/// 裸主机:`www.x.com` 或「已知 TLD 结尾的 a.b 形态」,且必须出现在文本开头或分隔符之后
/// (分隔符 = 空白 / 引号 / 反引号 / 括号 / 斜杠 / 中文标点),避免从单词中间截取。
static RE_BARE_HOST: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r##"(?i)(?:^|[\s/\\(`\[{<"'，。、;；])((?:www\.)?(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,})"##)
        .expect("RE_BARE_HOST 静态正则合法")
});

/// 目标指代未解析:「这个/该/那个/此/本 + 网站类名词」。
///
/// **刻意只覆盖 web 目标**,不含「这个项目 / 这个文件 / 这个目录」——
/// 后者在工作目录/工作区快照里有天然先行词(当前项目),机械判定为"不可解析"会大量误伤编码任务;
/// 桌面软件类指代(「这个软件」)交给 L1 的 LLM 通道(`target_status`)判断。
static RE_UNRESOLVED_WEB_REFERENCE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(这个|该|那个|此|本|上述|前述)\s*(网站|站点|网址|网站地址|门户网站|site|website)")
        .expect("RE_UNRESOLVED_WEB_REFERENCE 静态正则合法")
});

/// 从 URL 字符串提取**原始主机**(小写,不归一;纯函数,可单测)。
///
/// 无 scheme 时自动补 `https://` 再解析(用户常写裸主机)。
/// 返回 `None` 的两种情形都刻意偏向 fail-open:
///
/// 1. **authority 段非纯 ASCII** —— `url::Url::parse("https://不是一个网址")` 会成功,
///    并把中文 IDNA 编码成 `xn--4gqza7fw65bcysbp8a`,让垃圾串伪装成合法主机。
///    对锚点机制而言这是**误阻断合法任务**的危险方向,因此在解析前就用原文 ASCII 判定拦掉;
/// 2. **结构不像主机** —— 无点且非 `localhost` / 空标签 / 末段非纯字母 TLD。
pub fn parse_url_host(url: &str) -> Option<String> {
    let trimmed = url
        .trim()
        .trim_matches(|c| c == '`' || c == '<' || c == '>' || c == '"' || c == '\'');
    if trimmed.is_empty() {
        return None;
    }
    // 解析前先取 authority 段(第一个 / ? # 之前),用**原文**做 ASCII 判定 ——
    // 解析后再判就晚了:punycode 结果永远是 ASCII。
    let authority_source = match trimmed.find("://") {
        Some(i) => &trimmed[i + 3..],
        None => trimmed,
    };
    let authority = authority_source.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || !authority.is_ascii() {
        return None;
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = url::Url::parse(&with_scheme).ok()?;
    // 去掉末尾根点(FQDN 形态 `x.com.`)
    let host = parsed.host_str()?.trim().to_lowercase();
    let host = host.trim_end_matches('.').to_string();
    if !host_looks_like_real_hostname(&host) {
        return None;
    }
    Some(host)
}

/// 主机字面量结构是否可信:纯 ASCII + (IP 字面量 | `localhost` | 含点且末段为字母 TLD)。
fn host_looks_like_real_hostname(host: &str) -> bool {
    if host.is_empty() || !host.is_ascii() {
        return false;
    }
    if host.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    // IPv6 字面量(url crate 已去方括号,故用冒号数判定)
    if host.matches(':').count() >= 2 {
        return true;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.iter().any(|l| l.is_empty()) {
        return false;
    }
    if labels.len() < 2 {
        return host == "localhost";
    }
    labels
        .last()
        .map(|l| l.chars().all(|c| c.is_ascii_alphabetic()))
        .unwrap_or(false)
}

/// 从 URL 字符串提取主机并归一到**可注册域**(纯函数,可单测)。
///
/// - `https://www.anthropic.com/news` → `anthropic.com`
/// - `https://news.anthropic.com:8443/a?b=c#d` → `anthropic.com`
/// - `www.example.com.cn/x` → `example.com.cn`(命中多段公共后缀)
/// - `http://192.168.1.10:8080/` → `192.168.1.10`(IP 原样返回,不做段裁剪)
/// - 非法串 / 无主机 / 非 ASCII 垃圾串 → `None`
pub fn registrable_domain_of_url(url: &str) -> Option<String> {
    parse_url_host(url).map(|host| registrable_domain_of_host(&host))
}

/// 把已归一的小写主机裁到可注册域(纯函数,可单测)。
///
/// IP 字面量(含 IPv6)原样返回 —— 对 IP 做「末两段」裁剪没有意义,
/// 且锚点里出现 IP 时精确匹配才是正确语义。
pub fn registrable_domain_of_host(host: &str) -> String {
    let host = host.trim().to_lowercase();
    let host = host.trim_end_matches('.').to_string();
    if host.is_empty() {
        return host;
    }
    // IPv6 字面量(可能带方括号)
    if host.starts_with('[') || host.matches(':').count() >= 2 {
        return host;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() <= 2 {
        return host;
    }
    // 全数字末段 = IPv4 → 原样返回
    if labels.last().map(|l| l.chars().all(|c| c.is_ascii_digit())).unwrap_or(false) {
        return host;
    }
    let last_two = format!("{}.{}", labels[labels.len() - 2], labels[labels.len() - 1]);
    if MULTI_PART_PUBLIC_SUFFIXES.contains(&last_two.as_str()) {
        return format!("{}.{}", labels[labels.len() - 3], last_two);
    }
    last_two
}

/// host 是否落在 anchor 域内(后缀匹配,纯函数)。
///
/// `news.anthropic.com` ∈ `anthropic.com` ✅;`anthropic.com` ∈ `anthropic.com` ✅;
/// `ithome.com` ∉ `anthropic.com` ✅;`notanthropic.com` ∉ `anthropic.com` ✅
/// (**必须是点边界后缀匹配,不能退化成子串匹配**)。
pub fn host_matches_anchor_domain(host: &str, anchor_domain: &str) -> bool {
    let host = registrable_domain_of_host(host);
    let anchor = registrable_domain_of_host(anchor_domain);
    if host.is_empty() || anchor.is_empty() {
        return false;
    }
    host == anchor || host.ends_with(&format!(".{anchor}"))
}

/// 扫描文本中的全部主机并归一去重(纯函数,可单测)。
///
/// 双通道:`http(s)://` 完整 URL(主) + 裸主机(次,受 [`KNOWN_TLDS`] 白名单约束,
/// 避免把 `src/agent/mod.rs` 这类源码路径误判成域名)。
pub fn extract_hosts_from_text(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in RE_SCHEME_URL.find_iter(text) {
        if let Some(d) = registrable_domain_of_url(m.as_str()) {
            if !out.contains(&d) {
                out.push(d);
            }
        }
    }
    for cap in RE_BARE_HOST.captures_iter(text) {
        let Some(raw) = cap.get(1) else { continue };
        let candidate = raw.as_str().trim().to_lowercase();
        if candidate.is_empty() || !bare_host_looks_real(&candidate) {
            continue;
        }
        let domain = registrable_domain_of_host(&candidate);
        if !domain.is_empty() && !out.contains(&domain) {
            out.push(domain);
        }
    }
    out
}

/// 裸主机可信度判定:TLD 必须在白名单内(排除 `mod.rs` / `Cargo.toml` 等文件路径)。
fn bare_host_looks_real(candidate: &str) -> bool {
    if candidate.starts_with("www.") {
        return true;
    }
    let Some(tld) = candidate.rsplit('.').next() else {
        return false;
    };
    KNOWN_TLDS.contains(&tld)
}

/// 检测「目标指代未解析」:返回命中的原文片段(纯函数,可单测)。
///
/// 只判定指代是否存在;**是否真的无先行词**由调用方结合全上下文主机扫描决定
/// (见 [`TargetAnchor::extract_from_context`])——这样多轮对话里
/// 「这个网站」若前轮出现过 URL 就不会误触发。
pub fn detect_unresolved_target_reference(text: &str) -> Option<String> {
    let m = RE_UNRESOLVED_WEB_REFERENCE.find(text)?;
    Some(m.as_str().to_string())
}

/// 一次目标越界阻断的详情(供工具层构造 code=6001 信封)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorViolation {
    /// 请求 URL 归一后的主机
    pub host: String,
    /// 锚点允许的主机列表(信封里回给 LLM,让它无需猜)
    pub allowed_hosts: Vec<String>,
    /// 锚点证据(用户原文里的 URL 片段,供 LLM 与用户对账)
    pub evidence: String,
}

/// 任务锚点:从用户原文机械抽取的目标硬约束。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TargetAnchor {
    /// 锚定主机(可注册域,小写)
    pub hosts: Vec<String>,
    /// 原文中出现的完整 URL(逐字保留,供提示词回显与审计)
    pub raw_urls: Vec<String>,
    /// 目标指代未解析:存在「这个网站」类指代,但全可见上下文找不到任何主机
    pub unresolved_reference: bool,
    /// 命中未解析指代的原文片段(供澄清问题逐字引用)
    pub unresolved_evidence: String,
}

impl TargetAnchor {
    /// 从单段文本抽取锚点(纯函数)。
    ///
    /// `unresolved_reference` 在本函数内即判定完成(文本内无主机 + 命中指代);
    /// 多消息聚合请用 [`TargetAnchor::extract_from_context`]。
    pub fn extract_from_text(text: &str) -> Self {
        let hosts = extract_hosts_from_text(text);
        let raw_urls: Vec<String> = RE_SCHEME_URL
            .find_iter(text)
            .map(|m| m.as_str().trim_end_matches(['.', ',', ')', ']']).to_string())
            .filter(|u| !u.is_empty())
            .collect();
        let reference = detect_unresolved_target_reference(text);
        let unresolved_reference = reference.is_some() && hosts.is_empty();
        Self {
            hosts,
            raw_urls,
            unresolved_reference,
            unresolved_evidence: reference.unwrap_or_default(),
        }
    }

    /// 从可见上下文抽取锚点:**用户消息优先**,系统注入段(项目上下文/历史摘要)也参与扫描。
    ///
    /// `unresolved_reference` 只在「全部可见文本里零主机」时为真 ——
    /// 因此多轮对话中前轮出现过的 URL 会自动消解本轮的「这个网站」指代,不误触发澄清门。
    pub fn extract_from_context(messages: &[ChatMessage]) -> Self {
        if !target_anchor_enabled() {
            return Self::default();
        }
        // 用户消息(真实意图)与其余可见文本分开收集:指代检测只看用户消息,
        // 主机扫描看全部(历史摘要里提到的 URL 同样能消解指代)。
        let mut user_text = String::new();
        let mut all_text = String::new();
        for m in messages {
            let t = m.content_text();
            if t.trim().is_empty() {
                continue;
            }
            all_text.push_str(&t);
            all_text.push('\n');
            if m.role == Role::User {
                user_text.push_str(&t);
                user_text.push('\n');
            }
        }
        let hosts = extract_hosts_from_text(&all_text);
        let raw_urls: Vec<String> = RE_SCHEME_URL
            .find_iter(&user_text)
            .map(|m| m.as_str().trim_end_matches(['.', ',', ')', ']']).to_string())
            .filter(|u| !u.is_empty())
            .collect();
        let reference = detect_unresolved_target_reference(&user_text);
        let unresolved_reference = reference.is_some() && hosts.is_empty();
        Self {
            hosts,
            raw_urls,
            unresolved_reference,
            unresolved_evidence: reference.unwrap_or_default(),
        }
    }

    /// 锚点是否为空(空 = 用户未指定目标 → **不做任何导航阻断**)。
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    /// host 是否落在锚点内。空锚点返回 `true`(不阻断开放任务)。
    pub fn contains_host(&self, host: &str) -> bool {
        if self.hosts.is_empty() {
            return true;
        }
        self.hosts.iter().any(|a| host_matches_anchor_domain(host, a))
    }

    /// 渲染注入提示词的约束段(空锚点且无未解析指代时返回 `None`,零 token 开销)。
    pub fn render_prompt_section(&self) -> Option<String> {
        if self.is_empty() && !self.unresolved_reference {
            return None;
        }
        let mut out = String::from("【任务锚点 · 目标硬约束(系统从用户原文机械抽取,不可协商)】\n");
        if !self.hosts.is_empty() {
            out.push_str(&format!("  允许的目标站点: {}\n", self.hosts.join(" , ")));
            if !self.raw_urls.is_empty() {
                let shown: Vec<&str> = self.raw_urls.iter().take(3).map(|s| s.as_str()).collect();
                out.push_str(&format!("  用户原文 URL: {}\n", shown.join(" ")));
            }
            out.push_str(
                "  规则: 本任务的全部网页操作只允许落在上述站点内。目标站点不可达\n\
                 (超时/连接失败/404/需登录)时,**唯一正确动作是如实报告失败并结束**;\n\
                 严禁改用其它站点、搜索引擎、缓存或名称相似的替代品 —— 可以失败,不可以乱跑。\n\
                 MCP_Web_Use action=open 返回 code=6001 表示你正在偏离锚点,必须立即停止该路径。",
            );
        }
        if self.unresolved_reference {
            out.push_str(&format!(
                "\n  ⚠ 目标未指明: 用户原文出现指代「{}」,但全上下文找不到任何 URL 或站点名。\n\
                 这是**不可猜测**的信息缺失:禁止用侦察工具去猜用户指哪个网站,\n\
                 禁止自行挑一个站点开始操作;直接返回「需要用户澄清目标」并写明缺什么。",
                self.unresolved_evidence
            ));
        }
        Some(out)
    }
}

/// 构造回给用户的**澄清消息**(第 128 轮 L1 澄清门的输出文本,纯函数可单测)。
///
/// 设计要点:
/// 1. **逐字回显用户输入** —— 让用户立刻看出"我发的和它收到的不一样"(多行粘贴被
///    终端截断时,这一行就是最直接的证据);
/// 2. **明说不猜** —— 对齐用户要求「可以失败,但是不能乱跑」;
/// 3. **给出可操作的补充方式** —— 不是笼统的"请提供更多信息",而是列出具体缺什么;
/// 4. `llm_question` 非空时优先采用(Yolo 自己组织的问题更贴合具体任务),
///    机械模板作为兜底,保证 LLM 没填也有可用文案。
pub fn build_clarification_message(
    anchor: &TargetAnchor,
    user_prompt: &str,
    llm_question: Option<&str>,
) -> String {
    let prompt_echo: String = user_prompt.trim().chars().take(200).collect();
    let prompt_suffix = if user_prompt.trim().chars().count() > 200 {
        "…"
    } else {
        ""
    };
    let mut out = String::from("需要你先明确目标才能开始执行 —— 我不会替你猜一个网站/文件/应用。\n\n");
    out.push_str(&format!("你的输入:「{prompt_echo}{prompt_suffix}」\n"));
    if !anchor.unresolved_evidence.is_empty() {
        out.push_str(&format!(
            "未解析的指代:「{}」—— 上下文里找不到任何 URL 或站点名。\n",
            anchor.unresolved_evidence
        ));
    }
    if let Some(q) = llm_question.map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str(&format!("\n{q}\n"));
    } else {
        out.push_str("\n请补充其一:\n");
        out.push_str("  • 目标 URL(优先,如 https://www.anthropic.com/)\n");
        out.push_str("  • 站点/应用名称(如「Anthropic」「IT之家」)\n");
    }
    out.push_str("\n补充后我会重新执行本任务。");
    // 多行粘贴被终端截断的提示:用户看到的输入回显与自己发的不一致时,这是最可能的原因
    if user_prompt.lines().count() <= 1 && anchor.unresolved_reference {
        out.push_str(
            "\n\n(提示:若你原本发的是多行提示词,这里只显示了 1 行,说明终端把多行粘贴\n\
             截断了 —— 请改用 `laew -f 提示词文件.md` 或单行重发。)",
        );
    }
    out
}

/// 校验一次 `open` / `navigate` 的目标 URL 是否越出任务锚点。
///
/// 返回 `Some(violation)` 表示应阻断(工具层构造 code=6001 信封);`None` 表示放行。
/// 放行条件(任一成立):开关关闭 / 无锚点 / 锚点为空 / host 在锚点内 / host 无法解析
/// (无法解析时 fail-open —— 防漂移机制自身不能成为新的失败面)。
pub fn check_open_against_target_anchor(url: &str) -> Option<AnchorViolation> {
    if !target_anchor_enabled() || !target_anchor_block_enabled() {
        return None;
    }
    let anchor = current_target_anchor()?;
    check_open_against_anchor(url, Some(&anchor))
}

/// [`check_open_against_target_anchor`] 的锚点显式传入版(纯函数,可单测)。
///
/// 拆出本变体是为了让越界判定**不依赖全局槽** —— 全局槽是进程级可变状态,
/// Rust 测试默认并发运行,多个用例同时读写它会互相干扰产生偶发失败
/// (同类问题见 `url_safety.rs` 对 `std::env::set_var` 的规避注释)。
/// 开关判定仍在公开入口做,本函数只管锚点比对语义。
pub fn check_open_against_anchor(url: &str, anchor: Option<&TargetAnchor>) -> Option<AnchorViolation> {
    let anchor = anchor.filter(|a| !a.is_empty())?;
    // 主机解析不出来一律 fail-open:防漂移机制自身绝不能成为新的失败面
    let host = parse_url_host(url)?;
    if anchor.contains_host(&host) {
        return None;
    }
    Some(AnchorViolation {
        host,
        allowed_hosts: anchor.hosts.clone(),
        evidence: anchor.raw_urls.first().cloned().unwrap_or_default(),
    })
}

/// 从工具调用日志里检测**目标漂移**(L4 验收门的机械判据,纯函数)。
///
/// 扫描 `tool_call_log` 各条 `args_json` 中出现的 URL 主机,返回锚点外的主机列表
/// (去重、按首次出现顺序)。锚点为空时恒返回空 —— 开放任务不判漂移。
///
/// 只看显式带 URL 的参数(`open` / `navigate` / `new_tab` / `download`),
/// 点击导航的落地页无法从参数预知,由 QC 结合 `final_url` 事后对账。
pub fn detect_target_drift(tool_call_args: &[(String, String)], anchor: &TargetAnchor) -> Vec<String> {
    if anchor.is_empty() {
        return Vec::new();
    }
    let mut drifted: Vec<String> = Vec::new();
    for (tool, args_json) in tool_call_args {
        // 只对浏览器工具做主机比对,避免 Bash 命令行里的无关 URL 误伤
        if !tool.contains("Web") && !tool.contains("web") {
            continue;
        }
        for host in extract_hosts_from_text(args_json) {
            if !anchor.contains_host(&host) && !drifted.contains(&host) {
                drifted.push(host);
            }
        }
    }
    drifted
}

// =================== 任务级全局槽 ===================
//
// 与 `BrowserManager::global()` / `HumanAssistHub::global()` 同构:laew 单进程单任务,
// 同任务内并发的 SubAgent 共享同一锚点,无需 task_local。

/// 当前任务锚点全局槽。
static CURRENT_TARGET_ANCHOR: OnceLock<Mutex<Option<TargetAnchor>>> = OnceLock::new();

fn anchor_slot() -> &'static Mutex<Option<TargetAnchor>> {
    CURRENT_TARGET_ANCHOR.get_or_init(|| Mutex::new(None))
}

/// 编排器在任务入口写入锚点(**每个任务都必须写,含空锚点**)。
///
/// 空值写入是必需的:TUI 多轮对话中任务 N 的锚点若残留到任务 N+1,
/// 会用旧站点错误阻断新任务。`None` = 无锚点 = 不做任何主机阻断。
pub fn set_current_target_anchor(anchor: Option<TargetAnchor>) {
    if !target_anchor_enabled() {
        if let Ok(mut guard) = anchor_slot().lock() {
            *guard = None;
        }
        return;
    }
    if let Ok(mut guard) = anchor_slot().lock() {
        *guard = anchor;
    }
}

/// 读取当前任务锚点(工具层 / trace 层 / QC 层共用)。
pub fn current_target_anchor() -> Option<TargetAnchor> {
    if !target_anchor_enabled() {
        return None;
    }
    anchor_slot().lock().ok()?.clone()
}

/// 任务级锚点作用域守卫(RAII):drop 时清空全局槽。
///
/// ## 为什么必须是 RAII
///
/// `handle_inner` 有 6 个以上 return 路径(直答短路 / 澄清短路 / 重试耗尽 /
/// 取消上抛 / 正常收口 / 硬错误),手工在每条路径上清锚点必然漏 ——
/// 而漏掉的后果是**跨任务污染**:TUI 多轮对话里任务 N 的 `anthropic.com`
/// 残留到任务 N+1,会用旧站点错误阻断新任务的正常导航。
/// 守卫随栈帧退出自动清理,取消/panic 路径同样生效。
pub struct TargetAnchorScopeGuard {
    _private: (),
}

impl Drop for TargetAnchorScopeGuard {
    fn drop(&mut self) {
        set_current_target_anchor(None);
    }
}

/// 安装任务锚点并返回作用域守卫(编排器任务入口调用,持有到任务结束)。
pub fn install_target_anchor(anchor: TargetAnchor) -> TargetAnchorScopeGuard {
    set_current_target_anchor(Some(anchor));
    TargetAnchorScopeGuard { _private: () }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== registrable_domain_of_url ==========

    #[test]
    fn registrable_domain_strips_www_and_path() {
        assert_eq!(
            registrable_domain_of_url("https://www.anthropic.com/news").as_deref(),
            Some("anthropic.com")
        );
        assert_eq!(
            registrable_domain_of_url("https://anthropic.com/").as_deref(),
            Some("anthropic.com")
        );
    }

    #[test]
    fn registrable_domain_handles_port_query_fragment() {
        assert_eq!(
            registrable_domain_of_url("https://news.anthropic.com:8443/a?b=c#d").as_deref(),
            Some("anthropic.com")
        );
    }

    #[test]
    fn registrable_domain_keeps_subdomain_boundary_correct() {
        // 子域归一到可注册域,但 notanthropic.com 必须是独立域
        assert_eq!(
            registrable_domain_of_url("https://notanthropic.com/").as_deref(),
            Some("notanthropic.com")
        );
    }

    #[test]
    fn registrable_domain_honors_multi_part_suffix() {
        assert_eq!(
            registrable_domain_of_url("https://www.example.com.cn/x").as_deref(),
            Some("example.com.cn")
        );
        assert_eq!(
            registrable_domain_of_url("https://a.b.co.uk/").as_deref(),
            Some("b.co.uk")
        );
    }

    #[test]
    fn registrable_domain_keeps_ip_literal() {
        assert_eq!(
            registrable_domain_of_url("http://192.168.1.10:8080/").as_deref(),
            Some("192.168.1.10")
        );
    }

    #[test]
    fn registrable_domain_accepts_bare_host_and_backticks() {
        assert_eq!(
            registrable_domain_of_url("www.anthropic.com").as_deref(),
            Some("anthropic.com")
        );
        assert_eq!(
            registrable_domain_of_url("`https://www.anthropic.com/`").as_deref(),
            Some("anthropic.com")
        );
    }

    #[test]
    fn registrable_domain_rejects_garbage() {
        assert_eq!(registrable_domain_of_url(""), None);
        assert_eq!(registrable_domain_of_url("   "), None);
        assert_eq!(registrable_domain_of_url("不是一个网址"), None);
    }

    // ========== parse_url_host / IDNA fail-open ==========

    #[test]
    fn parse_url_host_rejects_cjk_idna_disguise() {
        // url::Url::parse("https://不是一个网址") 会成功并 punycode 成 xn--… ——
        // 那会让垃圾串伪装成合法主机,进而在工具门**误阻断**合法任务。必须在解析前拦掉。
        assert_eq!(parse_url_host("不是一个网址"), None);
        assert_eq!(parse_url_host("https://不是一个网址"), None);
        assert_eq!(parse_url_host("https://打开网站"), None);
    }

    #[test]
    fn parse_url_host_accepts_real_forms() {
        assert_eq!(parse_url_host("https://www.anthropic.com/news").as_deref(), Some("www.anthropic.com"));
        assert_eq!(parse_url_host("http://192.168.1.10:8080/").as_deref(), Some("192.168.1.10"));
        assert_eq!(parse_url_host("www.ithome.com").as_deref(), Some("www.ithome.com"));
        assert_eq!(parse_url_host("http://localhost:3000/x").as_deref(), Some("localhost"));
    }

    #[test]
    fn parse_url_host_rejects_structurally_invalid() {
        assert_eq!(parse_url_host("https://singleword"), None);
        assert_eq!(parse_url_host("https://a..b.com"), None);
        assert_eq!(parse_url_host("https://x.123"), None, "末段非字母 TLD");
        assert_eq!(parse_url_host(""), None);
        assert_eq!(parse_url_host("``"), None);
    }

    #[test]
    fn parse_url_host_strips_wrapping_punctuation() {
        assert_eq!(parse_url_host("`https://www.anthropic.com/`").as_deref(), Some("www.anthropic.com"));
        assert_eq!(parse_url_host("<https://a.com>").as_deref(), Some("a.com"));
    }

    // ========== host_matches_anchor_domain ==========

    #[test]
    fn host_match_is_dot_bounded_not_substring() {
        assert!(host_matches_anchor_domain("news.anthropic.com", "anthropic.com"));
        assert!(host_matches_anchor_domain("anthropic.com", "anthropic.com"));
        assert!(host_matches_anchor_domain("www.anthropic.com", "anthropic.com"));
        // 关键:子串相似但不同域,必须拒绝
        assert!(!host_matches_anchor_domain("notanthropic.com", "anthropic.com"));
        assert!(!host_matches_anchor_domain("ithome.com", "anthropic.com"));
        assert!(!host_matches_anchor_domain("anthropic.com.evil.cn", "anthropic.com"));
    }

    #[test]
    fn host_match_rejects_empty_inputs() {
        assert!(!host_matches_anchor_domain("", "anthropic.com"));
        assert!(!host_matches_anchor_domain("anthropic.com", ""));
    }

    // ========== extract_hosts_from_text ==========

    #[test]
    fn extract_hosts_finds_scheme_urls_in_cjk_text() {
        let text = "1. 网站地址 `https://www.anthropic.com/` ，自动遍历相关的子页面";
        assert_eq!(extract_hosts_from_text(text), vec!["anthropic.com".to_string()]);
    }

    #[test]
    fn extract_hosts_dedupes_and_preserves_order() {
        let text = "https://a.com/x 然后 https://b.com 再 https://www.a.com/y";
        assert_eq!(
            extract_hosts_from_text(text),
            vec!["a.com".to_string(), "b.com".to_string()]
        );
    }

    #[test]
    fn extract_hosts_ignores_source_file_paths() {
        // 裸主机通道必须放过源码路径(TLD 不在白名单)
        let text = "看 src/agent/mod.rs 和 src/tui/input.rs 还有 Cargo.toml";
        assert!(extract_hosts_from_text(text).is_empty());
    }

    #[test]
    fn extract_hosts_accepts_bare_www_host() {
        assert_eq!(
            extract_hosts_from_text("打开 www.ithome.com 看看"),
            vec!["ithome.com".to_string()]
        );
    }

    #[test]
    fn extract_hosts_empty_text() {
        assert!(extract_hosts_from_text("").is_empty());
        assert!(extract_hosts_from_text("搜一下最新 Rust 新闻").is_empty());
    }

    // ========== detect_unresolved_target_reference ==========

    #[test]
    fn unresolved_reference_detects_incident_prompt() {
        // 本次事故的精确形态
        let text = "### 打开网站搜索，这个网站最新时间的 3个文章、新闻或是报道的信息，显示出来";
        assert_eq!(
            detect_unresolved_target_reference(text).as_deref(),
            Some("这个网站")
        );
    }

    #[test]
    fn unresolved_reference_covers_demonstratives() {
        for t in ["该网站的内容", "那个站点", "此网址", "本网站", "上述网站"] {
            assert!(detect_unresolved_target_reference(t).is_some(), "应命中: {t}");
        }
    }

    #[test]
    fn unresolved_reference_ignores_non_web_nouns() {
        // 项目/文件/目录有工作目录作天然先行词,不得触发(否则编码任务全被误伤)
        for t in ["修改这个项目的配置", "读一下这个文件", "在这个目录里创建", "把这个函数重构"] {
            assert!(
                detect_unresolved_target_reference(t).is_none(),
                "不应命中: {t}"
            );
        }
    }

    #[test]
    fn unresolved_reference_ignores_plain_web_task() {
        assert!(detect_unresolved_target_reference("搜一下最新 Rust 新闻").is_none());
        assert!(detect_unresolved_target_reference("打开 https://a.com 抓取").is_none());
    }

    // ========== TargetAnchor ==========

    #[test]
    fn anchor_from_full_user_prompt_locks_target_host() {
        let text = "### 打开网站搜索，这个网站最新时间的 3个文章\n\
                    1. 网站地址 `https://www.anthropic.com/` ，自动遍历子页面\n\
                    2. 文章内容是关于 AI、大模型、Agent";
        let anchor = TargetAnchor::extract_from_text(text);
        assert_eq!(anchor.hosts, vec!["anthropic.com".to_string()]);
        // 有 URL → 指代已解析,不得触发澄清门
        assert!(!anchor.unresolved_reference);
        assert!(anchor.contains_host("www.anthropic.com"));
        assert!(anchor.contains_host("news.anthropic.com"));
        assert!(!anchor.contains_host("ithome.com"), "事故中的改派站点必须被拒");
    }

    #[test]
    fn anchor_from_truncated_prompt_flags_unresolved() {
        // 事故实际进入管线的残缺输入
        let text = "### 打开网站搜索，这个网站最新时间的 3个文章、新闻或是报道的信息，显示出来";
        let anchor = TargetAnchor::extract_from_text(text);
        assert!(anchor.is_empty());
        assert!(anchor.unresolved_reference);
        assert_eq!(anchor.unresolved_evidence, "这个网站");
    }

    #[test]
    fn anchor_empty_hosts_allows_any_host() {
        // 开放任务:锚点为空 → 不阻断
        let anchor = TargetAnchor::extract_from_text("搜一下最新 Rust 新闻给我 3 条");
        assert!(anchor.is_empty());
        assert!(anchor.contains_host("anything.com"));
        assert!(anchor.contains_host("ithome.com"));
    }

    #[test]
    fn anchor_prompt_section_is_none_when_nothing_to_say() {
        let anchor = TargetAnchor::extract_from_text("搜一下最新 Rust 新闻");
        assert!(anchor.render_prompt_section().is_none());
    }

    #[test]
    fn anchor_prompt_section_lists_allowed_hosts_and_6001() {
        let anchor = TargetAnchor::extract_from_text("打开 https://www.anthropic.com/ 抓取");
        let section = anchor.render_prompt_section().expect("非空锚点应渲染约束段");
        assert!(section.contains("anthropic.com"));
        assert!(section.contains("6001"));
        assert!(section.contains("不可以乱跑"));
    }

    #[test]
    fn anchor_prompt_section_warns_on_unresolved_reference() {
        let anchor = TargetAnchor::extract_from_text("这个网站最新的 3 篇文章");
        let section = anchor.render_prompt_section().expect("未解析指代应渲染告警段");
        assert!(section.contains("目标未指明"));
        assert!(section.contains("这个网站"));
    }

    #[test]
    fn anchor_from_context_resolves_reference_across_turns() {
        // 多轮:前轮给过 URL → 本轮「这个网站」不应触发澄清门
        let messages = vec![
            ChatMessage::user("帮我看看 https://www.anthropic.com/ 的定价"),
            ChatMessage::user("这个网站的最新 3 篇文章列一下"),
        ];
        let anchor = TargetAnchor::extract_from_context(&messages);
        assert_eq!(anchor.hosts, vec!["anthropic.com".to_string()]);
        assert!(
            !anchor.unresolved_reference,
            "前轮已出现 URL,指代已消解,不得误触发澄清门"
        );
    }

    #[test]
    fn anchor_from_context_flags_unresolved_when_no_host_anywhere() {
        let messages = vec![ChatMessage::user("打开网站搜索，这个网站最新时间的 3个文章")];
        let anchor = TargetAnchor::extract_from_context(&messages);
        assert!(anchor.unresolved_reference);
    }

    #[test]
    fn anchor_from_context_ignores_empty_messages() {
        let messages = vec![ChatMessage::user("   "), ChatMessage::user("")];
        let anchor = TargetAnchor::extract_from_context(&messages);
        assert!(anchor.is_empty());
        assert!(!anchor.unresolved_reference);
    }

    // ========== check_open_against_target_anchor ==========

    // ========== check_open_against_anchor(纯函数,不碰全局槽) ==========
    //
    // 这些用例刻意走显式传锚点的纯函数变体:全局槽是进程级可变状态,
    // Rust 测试默认并发运行,读写它的用例之间会互相干扰产生偶发失败。
    // 全局槽本身的行为由下面唯一一个 `global_anchor_slot_*` 用例覆盖。

    fn anchored(host: &str) -> TargetAnchor {
        TargetAnchor {
            hosts: vec![host.to_string()],
            raw_urls: vec![format!("https://{host}/")],
            unresolved_reference: false,
            unresolved_evidence: String::new(),
        }
    }

    #[test]
    fn open_check_passes_when_no_anchor() {
        assert_eq!(check_open_against_anchor("https://ithome.com/", None), None);
    }

    #[test]
    fn open_check_blocks_off_anchor_host() {
        // 事故复现:锚点 anthropic.com,Agent 想改派 ithome.com
        let violation = check_open_against_anchor("https://www.ithome.com/", Some(&anchored("anthropic.com")))
            .expect("锚点外主机必须被阻断");
        assert_eq!(violation.host, "www.ithome.com");
        assert_eq!(violation.allowed_hosts, vec!["anthropic.com".to_string()]);
        // evidence 取锚点的首条原文 URL(anchored() 按 host 拼,无 www 前缀)
        assert_eq!(violation.evidence, "https://anthropic.com/");
    }

    #[test]
    fn open_check_blocks_the_other_incident_host_too() {
        // 事故里 Agent 先猜的站点同样必须被拦
        assert!(check_open_against_anchor(
            "https://news.ycombinator.com/",
            Some(&anchored("anthropic.com"))
        )
        .is_some());
    }

    #[test]
    fn open_check_allows_anchor_host_and_subdomain() {
        let a = anchored("anthropic.com");
        assert_eq!(check_open_against_anchor("https://www.anthropic.com/news", Some(&a)), None);
        assert_eq!(check_open_against_anchor("https://news.anthropic.com/", Some(&a)), None);
        assert_eq!(check_open_against_anchor("anthropic.com", Some(&a)), None, "裸主机也应放行");
    }

    #[test]
    fn open_check_fails_open_on_unparsable_url() {
        let a = anchored("anthropic.com");
        // 无法解析主机 → 放行(防漂移机制自身不能成为新失败面)
        assert_eq!(check_open_against_anchor("不是网址", Some(&a)), None);
        assert_eq!(check_open_against_anchor("", Some(&a)), None);
        assert_eq!(check_open_against_anchor("https://", Some(&a)), None);
    }

    #[test]
    fn open_check_allows_everything_with_empty_anchor() {
        // 用户没指定站点 → 锚点为空 → 不阻断(开放任务零影响)
        let empty = TargetAnchor::default();
        assert_eq!(check_open_against_anchor("https://news.ycombinator.com/", Some(&empty)), None);
        assert_eq!(check_open_against_anchor("https://www.ithome.com/", Some(&empty)), None);
    }

    // ========== detect_target_drift ==========

    #[test]
    fn target_drift_flags_off_anchor_web_tool_call() {
        let calls = vec![
            ("MCP_Web_Use".to_string(), r#"{"action":"open","url":"https://www.ithome.com/"}"#.to_string()),
        ];
        assert_eq!(
            detect_target_drift(&calls, &anchored("anthropic.com")),
            vec!["ithome.com".to_string()]
        );
    }

    #[test]
    fn target_drift_ignores_anchor_host_calls() {
        let calls = vec![
            ("MCP_Web_Use".to_string(), r#"{"action":"open","url":"https://www.anthropic.com/"}"#.to_string()),
            ("MCP_Web_Use".to_string(), r#"{"action":"inspect","page_id":"p_1"}"#.to_string()),
        ];
        assert!(detect_target_drift(&calls, &anchored("anthropic.com")).is_empty());
    }

    #[test]
    fn target_drift_skips_non_web_tools() {
        // Bash 命令行里的无关 URL 不应误判为漂移
        let calls = vec![
            ("Bash".to_string(), r#"{"command":"curl https://example.com/health"}"#.to_string()),
        ];
        assert!(detect_target_drift(&calls, &anchored("anthropic.com")).is_empty());
    }

    #[test]
    fn target_drift_never_fires_with_empty_anchor() {
        let calls = vec![
            ("MCP_Web_Use".to_string(), r#"{"action":"open","url":"https://www.ithome.com/"}"#.to_string()),
        ];
        assert!(detect_target_drift(&calls, &TargetAnchor::default()).is_empty());
    }

    #[test]
    fn target_drift_dedupes_multiple_off_anchor_calls() {
        let calls = vec![
            ("MCP_Web_Use".to_string(), r#"{"url":"https://www.ithome.com/a"}"#.to_string()),
            ("MCP_Web_Use".to_string(), r#"{"url":"https://ithome.com/b"}"#.to_string()),
        ];
        assert_eq!(
            detect_target_drift(&calls, &anchored("anthropic.com")),
            vec!["ithome.com".to_string()]
        );
    }

    // ========== build_clarification_message ==========

    #[test]
    fn clarification_message_echoes_user_input_verbatim() {
        // 逐字回显是关键:多行被截断时,用户一眼看出"我发的和它收到的不一样"
        let prompt = "### 打开网站搜索，这个网站最新时间的 3个文章、新闻或是报道的信息，显示出来";
        let anchor = TargetAnchor::extract_from_text(prompt);
        let msg = build_clarification_message(&anchor, prompt, None);
        assert!(msg.contains("打开网站搜索"), "{msg}");
        assert!(msg.contains("这个网站"), "应点出未解析的指代: {msg}");
        assert!(msg.contains("不会替你猜"), "{msg}");
    }

    #[test]
    fn clarification_message_offers_concrete_options() {
        let anchor = TargetAnchor::extract_from_text("这个网站的最新文章");
        let msg = build_clarification_message(&anchor, "这个网站的最新文章", None);
        // 不能是笼统的"请提供更多信息"
        assert!(msg.contains("URL"), "{msg}");
        assert!(msg.contains("站点/应用名称"), "{msg}");
        assert!(msg.contains("anthropic.com"), "应给出可照抄的示例: {msg}");
    }

    #[test]
    fn clarification_message_prefers_llm_question() {
        let anchor = TargetAnchor::extract_from_text("这个网站的文章");
        let msg = build_clarification_message(
            &anchor,
            "这个网站的文章",
            Some("请问您要抓取的是哪个网站?给出域名即可。"),
        );
        assert!(msg.contains("请问您要抓取的是哪个网站"), "{msg}");
        // LLM 已给出问题时不再叠加机械模板的选项清单
        assert!(!msg.contains("站点/应用名称"), "{msg}");
    }

    #[test]
    fn clarification_message_blank_llm_question_falls_back() {
        let anchor = TargetAnchor::extract_from_text("这个网站的文章");
        let msg = build_clarification_message(&anchor, "这个网站的文章", Some("   "));
        assert!(msg.contains("站点/应用名称"), "空白问题应退回机械模板: {msg}");
    }

    #[test]
    fn clarification_message_hints_paste_truncation_for_single_line() {
        // 单行 + 未解析指代 = 多行粘贴被截断的高概率形态,主动提示用 -f 规避
        let anchor = TargetAnchor::extract_from_text("这个网站最新的 3 篇文章");
        let msg = build_clarification_message(&anchor, "这个网站最新的 3 篇文章", None);
        assert!(msg.contains("laew -f"), "{msg}");
        assert!(msg.contains("截断"), "{msg}");
    }

    #[test]
    fn clarification_message_no_truncation_hint_for_multiline_input() {
        let prompt = "这个网站最新的 3 篇文章\n主题要 AI 相关\n给出 URL";
        let anchor = TargetAnchor::extract_from_text(prompt);
        let msg = build_clarification_message(&anchor, prompt, None);
        assert!(!msg.contains("laew -f"), "多行输入不该提示截断: {msg}");
    }

    #[test]
    fn clarification_message_truncates_very_long_prompt_echo() {
        // 回显上限 200 字符:尾部标记必须被截掉,否则超长提示词会把澄清问题淹没
        let tail_marker = "TAIL_MARKER_XYZ";
        let long = format!("这个网站 {}{tail_marker}", "很长的补充说明".repeat(60));
        let anchor = TargetAnchor::extract_from_text(&long);
        let msg = build_clarification_message(&anchor, &long, None);
        assert!(msg.contains('…'), "超长输入应有截断省略号: {msg}");
        assert!(
            !msg.contains(tail_marker),
            "回显必须截断,不得带出原文尾部: {msg}"
        );
        assert!(msg.contains("这个网站"), "回显应保留开头: {msg}");
    }

    #[test]
    fn clarification_message_keeps_short_prompt_echo_intact() {
        // 未超上限时逐字完整回显(不加省略号)—— 用户要能确认"它收到的就是我发的"
        let prompt = "### 打开网站搜索，这个网站最新时间的 3个文章";
        let anchor = TargetAnchor::extract_from_text(prompt);
        let msg = build_clarification_message(&anchor, prompt, None);
        assert!(msg.contains(prompt.trim()), "短输入应逐字回显: {msg}");
        assert!(!msg.contains('…'), "{msg}");
    }

    // ========== 全局槽与 RAII 守卫 ==========
    //
    // ⚠ 本文件里**只有这一个**用例读写进程级全局槽,且内部串行断言。
    // 全局槽是可变共享状态,Rust 测试默认多线程并发运行 —— 若拆成多个 #[test],
    // 它们会互相覆盖对方刚写入的锚点,产生"单跑通过、全量偶发失败"的脏测试。
    // 需要测锚点比对语义时一律用纯函数变体(`check_open_against_anchor` /
    // `collect_failure_signals_with_anchor` / `detect_target_drift`),不要碰全局。

    #[test]
    fn global_anchor_slot_and_scope_guard_lifecycle() {
        // (1) 写入 → 读回
        set_current_target_anchor(None);
        set_current_target_anchor(Some(anchored("example.com")));
        let got = current_target_anchor().expect("写入后应可读回");
        assert_eq!(got.hosts, vec!["example.com".to_string()]);

        // (2) 显式清空
        set_current_target_anchor(None);
        assert!(current_target_anchor().is_none());

        // (3) RAII 守卫:作用域内可读,drop 后必须自动清空。
        //     否则 TUI 多轮对话里任务 N 的锚点会残留并错误阻断任务 N+1。
        {
            let _guard = install_target_anchor(anchored("anthropic.com"));
            assert_eq!(
                current_target_anchor().expect("作用域内应可读").hosts,
                vec!["anthropic.com".to_string()]
            );
            // 守卫存续期间,工具门按全局槽生效(端到端一致性)
            assert!(check_open_against_target_anchor("https://www.ithome.com/").is_some());
            assert!(check_open_against_target_anchor("https://www.anthropic.com/").is_none());
        }
        assert!(
            current_target_anchor().is_none(),
            "守卫 drop 后必须清空全局槽"
        );

        // (4) 提前 return(取消 / 错误上抛路径)同样必须清理
        fn task_with_early_return(fail: bool) -> Option<TargetAnchor> {
            let _guard = install_target_anchor(anchored("example.com"));
            if fail {
                return None; // 提前返回,守卫随栈帧 drop
            }
            current_target_anchor()
        }
        assert!(task_with_early_return(false).is_some());
        assert!(current_target_anchor().is_none(), "正常返回后须清空");
        assert!(task_with_early_return(true).is_none());
        assert!(current_target_anchor().is_none(), "提前返回后仍须清空");
    }
}
