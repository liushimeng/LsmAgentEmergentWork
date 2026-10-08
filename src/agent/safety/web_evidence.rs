//! 网页取证纪律(第 131 轮)—— 禁止用 Bash 探测/抓取**任务目标站点**。
//!
//! 实测事故(`llaew_20261008_124929.log`):`MCP_Web_Use(action=open)` 返回
//! `code=2001 net::ERR_CONNECTION_RESET` 后,SubAgent-Work 自行「降级」成
//! `curl / ping / nc / netstat / traceroute` 反复探同一台主机,28 次 Bash 对 8 次
//! MCP_Web_Use,烧掉大半迭代预算,既没登录也没产出结论。
//!
//! 与既有两条纪律同源:
//! - `prompt` 侧 `MCP_WEB_USE_PROMPT_SECTION` 第 10 条「反伪造红线」:禁止用 Bash
//!   手写本应由 MCP_Web_Use 产出的截图/抓取证据(QC 会对账工具轨迹);
//! - `target_anchor` 第 128 轮范围约束(L3):浏览器显式 URL 动作越界返回 6001。
//!
//! 本模块是**运行时兜底**:提示词说了模型仍可能绕过(尤其在 open 连续失败、
//! 迭代预算吃紧时),故对「命中网络取证命令 且 命令中的 host 落在任务锚点域内」
//! 的 Bash 调用直接拒绝,并把 Agent 推回 MCP_Web_Use。
//!
//! ## 口径(窄口径,避免误伤正常开发任务)
//!
//! 拒绝需**同时**满足 5 条,任一不满足即放行:
//!
//! 1. 开关 [`web_evidence_enabled`] 未关(默认开,与 `LAEW_TARGET_ANCHOR` 同构);
//! 2. 工具调用是 `Bash`(其它工具不管);
//! 3. 当前 Agent 工具面**含** `MCP_Web_Use` —— 不含就没有替代路径,拦了等于死路;
//! 4. 命令行命中网络取证模式([`detect_network_evidence_command`]);
//! 5. 命令行出现的 host 落在任务锚点域内([`hosts_in_command`] + [`TargetAnchor`])。
//!
//! 于是开发任务里的 `curl http://localhost:8080/health`、对非任务站点的
//! `ping example.com` 一律放行 —— 只在「拿 Bash 去顶替浏览器工具的活」时才拦。
//!
//! 开关:`LAEW_WEB_EVIDENCE=off|0|false|no` 关闭(默认开启)。

use crate::agent::safety::target_anchor::{
    extract_hosts_from_text, host_matches_anchor_domain, TargetAnchor,
};

/// 网络取证命令的程序名单(小写,按 token 边界匹配)。
///
/// 刻意**不含** `python` / `node` 等通用解释器 —— 它们既能跑本地脚本也能发网络请求,
/// 按名字拦会误伤大量正常开发任务;真正要治的是「拿 shell 网络工具顶替 MCP_Web_Use」。
const NETWORK_EVIDENCE_BINARIES: &[&str] = &[
    "curl", "wget", "nc", "ncat", "netcat", "telnet", "ping", "ping6", "traceroute", "traceroute6",
    "tracepath", "arp", "nmap", "http", "https", "httpie", "httping", "dig", "nslookup", "host",
    "openssl",
];

/// 一次被拦下的 Bash 网络取证调用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceViolation {
    /// 命令里命中的任务目标 host(第一个命中项)。
    pub host: String,
    /// 原始命令行(截断后用于回显)。
    pub command: String,
    /// 任务锚点允许的主机列表(给模型指路)。
    pub allowed_hosts: Vec<String>,
}

/// `LAEW_WEB_EVIDENCE` 总开关:默认开启,`off/0/false/no` 关闭。
pub fn web_evidence_enabled() -> bool {
    !matches!(
        std::env::var("LAEW_WEB_EVIDENCE")
            .unwrap_or_default()
            .trim()
            .to_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
}

/// 命令行是否命中网络取证模式(纯函数,可单测)。
///
/// 只认**命令位**上的程序名:首个 token,或前置分隔符(`|` `;` `&` `(` 换行)之后的
/// token。避免 `echo 'run curl later'` / `cat notes/curl.md` 这类把网络工具名
/// 当普通参数或文件名的情况被误判。
pub fn detect_network_evidence_command(cmd: &str) -> bool {
    command_position_tokens(cmd)
        .iter()
        .any(|tok| NETWORK_EVIDENCE_BINARIES.contains(&tok.as_str()))
}

/// 命令行里出现的 host(纯函数,可单测)。
///
/// 双通道,与任务锚点口径对齐但更全:
/// - 复用 [`extract_hosts_from_text`](crate::agent::safety::target_anchor::extract_hosts_from_text)
///   的 URL / 裸域名抽取(带 TLD 白名单);
/// - 额外补 **IP 字面量**通道 —— `nc -z 10.255.159.58 20122`、`ping 172.27.173.6`
///   这类内网诊断**不带 scheme**,裸主机通道因无 TLD 不会命中,而它们恰恰是实测事故里
///   Agent 用的主力命令,漏掉等于门形同虚设。
pub fn hosts_in_command(cmd: &str) -> Vec<String> {
    let mut out = extract_hosts_from_text(cmd);
    for ip in ip_literals(cmd) {
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
    out
}

/// 扫描文本中的 IPv4 字面量(纯函数)。
///
/// 逐段校验四段均为 0~255 的十进制数,避免 `1.2.3.4.5` / `v1.2.3` 之类被当 IP
/// (后者 `extract_hosts_from_text` 已按 TLD 白名单放过,这里同理收紧)。
fn ip_literals(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        // 左侧不得紧邻字母/数字/点(避免把 a1.2.3.4 的片段切出来)
        if i > 0 && (chars[i - 1].is_ascii_alphanumeric() || chars[i - 1] == '.') {
            i += 1;
            continue;
        }
        let mut parts: Vec<String> = Vec::new();
        let mut j = i;
        let mut valid = true;
        while parts.len() < 4 {
            let start = j;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            if j == start || j - start > 3 {
                valid = false;
                break;
            }
            let part: String = chars[start..j].iter().collect();
            if part.parse::<u16>().unwrap_or(999) > 255 {
                valid = false;
                break;
            }
            parts.push(part);
            if parts.len() < 4 {
                if j < chars.len() && chars[j] == '.' {
                    j += 1;
                } else {
                    valid = false;
                    break;
                }
            }
        }
        // 右侧不得紧邻点或数字(排除 1.2.3.4.5 / 1.2.3.45)
        let right_ok = j == chars.len() || (chars[j] != '.' && !chars[j].is_ascii_digit());
        if valid && right_ok {
            let ip = parts.join(".");
            if !out.contains(&ip) {
                out.push(ip);
            }
        }
        i = if valid { j } else { i + 1 };
    }
    out
}

/// Bash 命令是否应被拦截(纯函数核心,`has_web_use` 由调用方从工具面传入)。
///
/// 返回 `None` = 放行。锚点为空 / 命令无 host / 未命中取证程序 一律放行 ——
/// 防漂移机制自身绝不能成为新的失败面(与 `check_open_against_anchor` 同哲学)。
pub fn check_bash_against_web_evidence(
    cmd: &str,
    anchor: Option<&TargetAnchor>,
    has_web_use: bool,
) -> Option<EvidenceViolation> {
    if !has_web_use {
        return None;
    }
    let anchor = anchor.filter(|a| !a.is_empty())?;
    if !detect_network_evidence_command(cmd) {
        return None;
    }
    let host = hosts_in_command(cmd)
        .into_iter()
        .find(|h| anchor.hosts.iter().any(|a| host_matches_anchor_domain(h, a)))?;
    let mut command: String = cmd.chars().take(200).collect();
    if cmd.chars().count() > 200 {
        command.push_str("…");
    }
    Some(EvidenceViolation {
        host,
        command,
        allowed_hosts: anchor.hosts.clone(),
    })
}

/// 拒绝文案(与 `tool_exec` 里 `ToolNotFound` 的「可用工具边界」话术同风格):
/// 说清「为什么不行 + 该用什么 + 不许再绕」。
pub fn denial_text(v: &EvidenceViolation) -> String {
    format!(
        "[网页取证纪律] 禁止用 Bash 对任务目标站点做网络探测或抓取:命令涉及 {host}。\
         命令: {command}\n\
         任务目标站点({allowed})的一切证据(DOM / 界面文字 / 截图 / 网络请求)只能来自 \
         MCP_Web_Use —— Quality-Check 会与执行轨迹对账,Bash 抓来的内容无法核验。\n\
         请改用 MCP_Web_Use:action=open 打开目标站点拿 page_id → action=inspect 读 DOM/文字 → \
         action=control 做交互。若 open 返回 2001(连接失败)/3001(未装浏览器),\
         说明目标不可达或环境缺失,**如实报告即可,不要改用 Bash 绕行、不要换站点替代**。",
        host = v.host,
        command = v.command,
        allowed = v.allowed_hosts.join(" / "),
    )
}

/// 只保留「命令位」上的 token:首个 token,或前置分隔符(`|` `;` `&` `(` 换行)之后的
/// token(允许 `&&` / `||` 之间夹空格)。路径前缀 `/usr/bin/curl` 归一为 `curl`。
fn command_position_tokens(cmd: &str) -> Vec<String> {
    let chars: Vec<char> = cmd.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    // 首段之前没有分隔符,`at_cmd_pos` 天然为 true
    let mut at_cmd_pos = true;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if matches!(c, '|' | ';' | '&' | '(' | ')' | '`' | '\n') {
            at_cmd_pos = true;
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len()
            && !chars[i].is_whitespace()
            && !matches!(chars[i], '|' | ';' | '&' | '(' | ')' | '`' | '\n')
        {
            i += 1;
        }
        if at_cmd_pos {
            let tok: String = chars[start..i].iter().collect();
            let base = tok.rsplit('/').next().unwrap_or(&tok).to_lowercase();
            if !base.is_empty() {
                out.push(base);
            }
            // 本 token 之后的字符只有可能是分隔符或空白;分隔符会把 at_cmd_pos 置回 true
            at_cmd_pos = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchored(hosts: &[&str]) -> TargetAnchor {
        TargetAnchor {
            hosts: hosts.iter().map(|h| h.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn 识别常见网络取证命令() {
        for cmd in [
            "curl -s http://10.255.159.58:20122/",
            "wget https://ithome.com",
            "nc -z -w 5 10.255.159.58 20122",
            "ping -c 3 10.255.159.58",
            "traceroute 10.255.159.58",
            "curl -sk https://172.27.173.6:20518/x | head -30",
            "echo x && nc -z 10.255.159.58 20122",
        ] {
            assert!(detect_network_evidence_command(cmd), "应识别为取证命令: {cmd}");
        }
    }

    #[test]
    fn 普通构建与文件命令不误判() {
        for cmd in [
            "cargo build --release",
            "ls -la && git status",
            "./rebuild_restart_app.sh",
            "python -c 'print(1)'",
            "cat notes/curl.md",
            "echo 'run curl later'",
            "mkdir -p .laew_tmp",
        ] {
            assert!(
                !detect_network_evidence_command(cmd),
                "不应判为取证命令: {cmd}"
            );
        }
    }

    #[test]
    fn 命令行host抽取覆盖裸IP与URL() {
        // 带 scheme 的 URL
        let hosts = hosts_in_command("curl -s -o /dev/null http://10.255.159.58:20122/");
        assert!(hosts.contains(&"10.255.159.58".to_string()), "{hosts:?}");
        // 实测事故主力形态:不带 scheme 的裸 IP(ping / nc)
        let bare = hosts_in_command("ping -c 3 8.8.8.8 ; nc -z -w 5 172.27.173.6 20518");
        assert!(bare.contains(&"8.8.8.8".to_string()), "{bare:?}");
        assert!(bare.contains(&"172.27.173.6".to_string()), "{bare:?}");
        // 非法 IP 片段不得被当成 host
        let bogus = hosts_in_command("curl http://1.2.3.4.5/x ; tar v1.2.3 ; echo 999.1.1.1");
        assert!(bogus.is_empty(), "{bogus:?}");
    }

    #[test]
    fn 裸IP取证命令被拦_覆盖实测事故形态() {
        let a = anchored(&["172.27.173.6"]);
        let v = check_bash_against_web_evidence(
            "ping -c 2 -t 3 172.27.173.6 2>&1 | tail -3; nc -z -w 5 172.27.173.6 20518",
            Some(&a),
            true,
        )
        .expect("裸 IP 探测应被拦");
        assert_eq!(v.host, "172.27.173.6");
    }

    #[test]
    fn 锚点内host的取证命令被拦() {
        let a = anchored(&["10.255.159.58"]);
        let v = check_bash_against_web_evidence(
            "curl -s -o /dev/null -w \"HTTP:%{http_code}\" http://10.255.159.58:20122/",
            Some(&a),
            true,
        )
        .expect("应被拦");
        assert_eq!(v.host, "10.255.159.58");
        assert_eq!(v.allowed_hosts, vec!["10.255.159.58".to_string()]);
        assert!(denial_text(&v).contains("MCP_Web_Use"));
    }

    #[test]
    fn 锚点外host放行_开发调试不受影响() {
        let a = anchored(&["10.255.159.58"]);
        // 用户任务目标是内网站点,顺手 curl 一下自己的本地服务 —— 放行
        assert!(
            check_bash_against_web_evidence("curl -s http://localhost:8080/health", Some(&a), true)
                .is_none()
        );
        // 探非任务站点也放行
        assert!(
            check_bash_against_web_evidence("curl -s https://www.baidu.com", Some(&a), true)
                .is_none()
        );
    }

    #[test]
    fn 无锚点或无Web工具或非取证命令一律放行() {
        let a = anchored(&["10.255.159.58"]);
        // 无锚点(开放任务)
        assert!(
            check_bash_against_web_evidence("curl http://10.255.159.58:20122/", None, true)
                .is_none()
        );
        // 无 MCP_Web_Use(没有替代路径,拦了就是死路)
        assert!(
            check_bash_against_web_evidence("curl http://10.255.159.58:20122/", Some(&a), false)
                .is_none()
        );
        // 非取证命令
        assert!(
            check_bash_against_web_evidence("cargo build", Some(&a), true).is_none()
        );
        // 命令里没有 host
        assert!(check_bash_against_web_evidence("curl --version", Some(&a), true).is_none());
    }

    #[test]
    fn 子域同级匹配按锚点域() {
        let a = anchored(&["cloud.example.com"]);
        let v = check_bash_against_web_evidence(
            "curl -s https://api.cloud.example.com/v1/login",
            Some(&a),
            true,
        );
        assert!(v.is_some(), "同锚点域下的兄弟子域应被拦");
    }

    #[test]
    fn 超长命令截断防刷屏() {
        let a = anchored(&["10.255.159.58"]);
        let long = format!(
            "curl http://10.255.159.58:20122/ {}",
            "x".repeat(500)
        );
        let v = check_bash_against_web_evidence(&long, Some(&a), true).expect("应被拦");
        assert!(v.command.chars().count() <= 201, "命令应被截断到 200 字 + 省略号");
    }
}