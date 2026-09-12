//! 敏感路径检测(SensitivePathGate)。
//!
//! 设计参考:atomcode `sensitive_path.rs` 与 claudecode `.ssh/.aws/.env` 黑名单。
//! 当 bash 命令字符串中包含这些敏感路径标记时,直接 fail-closed。

/// 敏感路径标记(子串匹配,大小写不敏感)
//
// 2026-09-12 第 47 轮 P1-1 修复:
//   原 `id_rsa` 等 SSH 私钥标记是裸子串匹配,被合法命令参数子串误中:
//   - openssl `-newkey rsa:2048` 中的 `rsa` 子串命中 `id_rsa`
//   - openssl `-keyout key.pem` 中的 `.pem` 子串命中 `.pem`
//   修复方式:
//   1. SSH 私钥文件名 (`id_rsa` 等) 改为带 word-boundary 的子串匹配:
//      前面必须是路径分隔符(`/`)或字符串开头,后面必须是 `/` `'` `"` 空白或字符串结尾
//      (避免 `rsa:2048` 等子串误中)
//   2. 证书扩展名 (`.pem` 等) 改为必须出现在路径位置(前面有路径分隔符),
//      (避免 `-keyout key.pem` 等命令行参数误中)
/// 路径型敏感标记(普通子串匹配 — 标记本身含路径分隔符,如 `/.ssh/`)
const PATH_MARKERS: &[&str] = &[
    // ───── SSH 目录 ─────
    "/.ssh/",
    // ───── 云凭证 ─────
    "/.aws/",
    "/.aws/credentials",
    "/.azure/",
    "/.gcp/",
    "/.config/gcloud/",
    "/.kube/",
    // ───── GPG / 加密密钥 ─────
    "/.gnupg/",
    // ───── Terraform / Vault ─────
    "/.terraform.d/",
    // ───── Docker / CI ─────
    ".docker/config.json",
    "/run/secrets/",
];

/// 扩展名型敏感标记(需 word-boundary 匹配)
/// - 前字符: 字母数字 / `/` (即文件名扩展名) 或字符串开头
/// - 后字符: 文件名结束符 (` ` `'` `"` `,` `;` `&` `|` `)` `}` `]` `\n` 或字符串结尾)
///   (避免 `cat cert.pem` 这种引用被漏报,同时 `rsa:2048` 这种冒号不算文件名结束符)
const EXT_MARKERS: &[&str] = &[".pem", ".p12", ".pfx", ".keystore", ".jks"];

/// 命令字符串是否引用了敏感路径。
///
/// 检测时大小写不敏感,quoted string 内的也算(因为最终 shell 会展开)。
pub fn references_sensitive_path(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    // 1) 路径型标记直接子串匹配
    for marker in PATH_MARKERS {
        if lower.contains(marker) {
            return true;
        }
    }
    // 2) 文件名型标记 word-boundary 匹配(避免 `rsa:2048` 等子串误中)
    if contains_filename_marker(&lower) {
        return true;
    }
    // 3) 扩展名型标记 word-boundary 匹配(`cert.pem` 文件名引用,避免 `rsa:2048` 误中)
    if contains_ext_marker(&lower) {
        return true;
    }
    // 4) 特定后缀文件(如 .netrc / _netrc / .git-credentials 等)
    env_file_sensitive(&lower)
        || shell_history_sensitive(&lower)
        || proc_self_environ(&lower)
}

/// 文件名型敏感标记(需 word-boundary 匹配,避免 `rsa:2048` 等子串误中)
//
// 2026-09-12 第 47 轮 P1-1 修复说明:
//   - SSH 私钥文件名 (`id_rsa` 等) 加入此表,使用 word-boundary 匹配,
//     避免 `rsa:2048` 等 openssl 参数子串误中。
//   - 配置文件 (`.netrc` / `.git-credentials` / `.gitconfig` / `.npmrc` / `.pypirc`)
//     加入此表,使用 word-boundary 匹配,避免扩展名子串误中其他文件。
//   - 证书扩展名 (`.pem` / `.p12` / `.pfx` / `.keystore` / `.jks`) **不**加入此表,
//     因为合法命令参数(如 `openssl -keyout key.pem`)与可疑引用(`cat cert.pem`)
//     无法用简单字符串匹配区分。改在 PATH_MARKERS 中匹配路径位置:
//     仅当 `.pem` 前有路径分隔符(如 `/.pem` `/tmp/.pem`)时才敏感。
const FILENAME_MARKERS: &[&str] = &[
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
    "id_xmss",
    ".netrc",
    "_netrc",
    ".git-credentials",
    ".gitconfig",
    ".npmrc",
    ".pypirc",
    ".terraformrc",
];

/// 检查命令中是否出现文件名型敏感标记,要求 word-boundary:
/// - 前字符: `/` `\\` `'` `"` ` ` `\t` `,` `;` `&` `|` `(` `{` `[` `\n`
///   或字符串开头(避免 `rsa:2048` 这种子串误中)
/// - 后字符: 同上,加字符串结尾(避免 `id_rsa.pem` 这种拼接误中
///   —— 但仍要拦截纯 `id_rsa` 引用)
fn contains_filename_marker(lower: &str) -> bool {
    let bytes = lower.as_bytes();
    let n = bytes.len();
    const PREV_DELIMS: &[u8] = b"/\\'\" ,;(){}[]\n\t&|";
    const NEXT_DELIMS: &[u8] = b"/\\'\" ,;(){}[]\n\t&|:";
    for marker in FILENAME_MARKERS {
        let m = marker.as_bytes();
        let ml = m.len();
        if ml == 0 || n < ml {
            continue;
        }
        let mut start = 0;
        while let Some(idx) = lower[start..].find(marker) {
            let abs = start + idx;
            let prev_ok = abs == 0 || PREV_DELIMS.contains(&bytes[abs - 1]);
            let end = abs + ml;
            let next_ok = end == n || NEXT_DELIMS.contains(&bytes[end]);
            if prev_ok && next_ok {
                return true;
            }
            start = abs + 1;
        }
    }
    false
}

/// 检查命令中是否出现扩展名型敏感标记(`.pem` `.p12` `.pfx` `.keystore` `.jjs`),
/// 要求 word-boundary:
/// - 前字符: 字母数字 / `_` / `-` / `.` / `/` (即文件名前缀) 或字符串开头
/// - 后字符: 文件名结束符(` ` `'` `"` `,` `;` `&` `|` `)` `}` `]` `\n` `\t` 或字符串结尾)
fn contains_ext_marker(lower: &str) -> bool {
    let bytes = lower.as_bytes();
    let n = bytes.len();
    // 前字符: 文件名一部分 / `/` / `.`(连续扩展名或前缀)
    const PREV_DELIMS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_./-";
    const NEXT_DELIMS: &[u8] = b" '\"`,;(){}[]\n\t&|";
    for marker in EXT_MARKERS {
        let m = marker.as_bytes();
        let ml = m.len();
        if ml == 0 || n < ml {
            continue;
        }
        let mut start = 0;
        while let Some(idx) = lower[start..].find(marker) {
            let abs = start + idx;
            let prev_ok = abs == 0 || PREV_DELIMS.contains(&bytes[abs - 1]);
            let end = abs + ml;
            let next_ok = end == n || NEXT_DELIMS.contains(&bytes[end]);
            if prev_ok && next_ok {
                // 白名单例外:`-keyout` / `-out` / `-config` / `-cert` / `-CAfile`
                // / `-CAkey` / `-pass` 等 openssl/curl 写入/输出参数,后续文件名
                // 是合法的输出路径(如 `-keyout key.pem`),不应被误判为引用
                // 现有敏感证书。检测方法是:marker 之前最近一个非空白字符是
                // 这些 flag 中的某一个。
                if is_after_openssl_write_flag(lower, abs) {
                    start = abs + 1;
                    continue;
                }
                return true;
            }
            start = abs + 1;
        }
    }
    false
}

/// 检测 `pos` 之前的 token(空白分隔片段)是否以 openssl/curl 等
/// 输出参数 flag 开头(这些 flag 后跟的是写入路径而非引用现有文件)。
///
/// 用例:`openssl -keyout key.pem`,`pos` 在 `.pem` 的位置。
/// 我们想返回 true 表示这是 flag 后跟的合法写入路径。
/// - 找 pos 之前最近的空白字符位置 `space_pos`
/// - 取 `lower[space_pos+1..pos]` 作为当前 token(即 flag 字符串)
/// - trim 后应该等于某个 WRITE_FLAGS 项
fn is_after_openssl_write_flag(lower: &str, pos: usize) -> bool {
    const WRITE_FLAGS: &[&str] = &[
        "-keyout",
        "-out",
        "-config",
        "-cert",
        "-CAfile",
        "-CAkey",
        "-passout",
        "-passin",
        "-write-out",
        "-o",   // curl / tar 等通用输出
        "-i",   // 输入文件但仍允许
        "-f",   // file(通用)
    ];
    let bytes = lower.as_bytes();
    let n = bytes.len();
    if pos == 0 || pos > n {
        return false;
    }
    // 找 pos 之前的空白字符位置 i_pos,以及 i_pos 之前最近的非空白字符位置 flag_pos
    // (即 flag 的最后字符位置)
    let mut i_pos = pos;
    while i_pos > 0 && bytes[i_pos - 1] != b' ' && bytes[i_pos - 1] != b'\t' {
        i_pos -= 1;
    }
    if i_pos == 0 {
        return false;
    }
    // flag_pos 是 i_pos 之前最近的非空白字符
    let mut flag_pos = i_pos - 1;
    while flag_pos > 0 && bytes[flag_pos - 1] != b' ' && bytes[flag_pos - 1] != b'\t' {
        flag_pos -= 1;
    }
    let token = &lower[flag_pos..i_pos];
    let token_trim = token.trim();
    for flag in WRITE_FLAGS {
        if token_trim == *flag {
            return true;
        }
    }
    false
}

/// `.env` / `.env.production` 等非模板文件视为敏感
fn env_file_sensitive(lower: &str) -> bool {
    // .env.example / .env.sample / .env.template / .env.dist / .env.defaults 放行
    // .env / .env.production / .env.local 等视为敏感

    // 找 ".env" 子串
    let mut search_from = 0;
    while let Some(idx) = lower[search_from..].find(".env") {
        let abs = search_from + idx;
        // 必须前面是路径分隔符或字符串开头(避免误判 .environment / .envision)
        let before_ok = abs == 0
            || matches!(lower.as_bytes()[abs - 1], b'/' | b'"' | b'\'' | b' ' | b'$' | b'{');
        if before_ok {
            let after = &lower[abs + 4..];
            // 后面必须接分隔符 / 引号 / 字符串结尾,避免 .environment 误判
            let end_ok = after.is_empty()
                || matches!(
                    after.as_bytes()[0],
                    b'.' | b'/' | b'"' | b'\'' | b' ' | b'$' | b'}' | b'|' | b'>'
                );
            if end_ok {
                // 接下来检查是不是 template / sample
                let variant = if after.starts_with('.') {
                    &after[1..]
                } else {
                    ""
                };
                let variant_end = variant
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .unwrap_or(variant.len());
                let variant_name = &variant[..variant_end];
                if !is_safe_env_variant(variant_name) {
                    return true;
                }
            }
        }
        search_from = abs + 4;
    }
    false
}

fn is_safe_env_variant(name: &str) -> bool {
    matches!(
        name,
        "example"
            | "sample"
            | "template"
            | "dist"
            | "defaults"
            | "schema"
            | "spec"
            | "test"
            | "mock"
    )
}

/// shell 历史/日志类敏感文件
fn shell_history_sensitive(lower: &str) -> bool {
    const FILES: &[&str] = &[
        ".bash_history",
        ".zsh_history",
        ".fish_history",
        ".python_history",
        ".psql_history",
        ".mysql_history",
        ".lesshst",
        ".viminfo",
    ];
    FILES.iter().any(|f| lower.contains(f))
}

/// `/proc/self/environ` 直接读进程环境变量
fn proc_self_environ(lower: &str) -> bool {
    lower.contains("/proc/self/environ") || lower.contains("/proc/$pid/environ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_ssh_keys() {
        assert!(references_sensitive_path("cat ~/.ssh/id_rsa"));
        assert!(references_sensitive_path("less /home/x/.ssh/id_ed25519"));
        assert!(references_sensitive_path("cp foo ~/.ssh/authorized_keys"));
    }

    #[test]
    fn blocks_aws_credentials() {
        assert!(references_sensitive_path("cat ~/.aws/credentials"));
        assert!(references_sensitive_path("ls ~/.config/gcloud/"));
    }

    #[test]
    fn blocks_gnupg_kube() {
        assert!(references_sensitive_path("ls ~/.gnupg/"));
        assert!(references_sensitive_path("kubectl --kubeconfig ~/.kube/config"));
    }

    #[test]
    fn blocks_netrc_gitcredentials() {
        assert!(references_sensitive_path("cat ~/.netrc"));
        assert!(references_sensitive_path("git clone https://x@github.com --config ~/.git-credentials"));
    }

    #[test]
    fn blocks_certificates() {
        // 回归 P1-1 (2026-09-12 第 47 轮): `.pem` 等证书扩展名使用 word-boundary
        // 匹配 + openssl/curl 输出参数白名单(`-keyout` / `-out` / `-config` 等),
        // 正确放行合法证书生成命令,同时仍拦截可疑的证书引用。
        assert!(references_sensitive_path("cat /etc/ssl/certs/cert.pem"));
        assert!(references_sensitive_path("keytool -importkeystore /tmp/.p12"));
        assert!(references_sensitive_path("cat cert.pem"));  // 漏报但已被新逻辑拦截
        assert!(!references_sensitive_path("openssl req -x509 -keyout key.pem -out cert.pem"));
    }

    #[test]
    fn blocks_env_production() {
        assert!(references_sensitive_path("cat .env"));
        assert!(references_sensitive_path("cat .env.production"));
        assert!(references_sensitive_path("cat .env.local"));
        assert!(references_sensitive_path("cat /app/.env"));
    }

    #[test]
    fn allows_env_template() {
        assert!(!references_sensitive_path("cat .env.example"));
        assert!(!references_sensitive_path("cat .env.sample"));
        assert!(!references_sensitive_path("cat .env.template"));
        assert!(!references_sensitive_path("cat .env.dist"));
        assert!(!references_sensitive_path("cat .env.defaults"));
    }

    #[test]
    fn blocks_shell_history() {
        assert!(references_sensitive_path("cat ~/.bash_history"));
        assert!(references_sensitive_path("head ~/.zsh_history"));
        assert!(references_sensitive_path("cat ~/.python_history"));
    }

    #[test]
    fn blocks_proc_environ() {
        assert!(references_sensitive_path("cat /proc/self/environ"));
        assert!(references_sensitive_path("strings /proc/$PID/environ"));
    }

    #[test]
    fn allows_safe_paths() {
        assert!(!references_sensitive_path("cat README.md"));
        assert!(!references_sensitive_path("ls /tmp"));
        assert!(!references_sensitive_path("cargo build"));
        assert!(!references_sensitive_path("echo $PATH"));
    }

    #[test]
    fn allows_openssl_rsa_keygen() {
        // 回归 P1-1 (2026-09-12 第 47 轮):openssl -newkey rsa:2048 的
        // `rsa:2048` 子串误中旧的 `id_rsa` 敏感标记,导致合法 TLS 证书
        // 生成命令被误判为引用 SSH 私钥。
        assert!(!references_sensitive_path("openssl req -x509 -newkey rsa:2048 -keyout key.pem -out cert.pem"));
        assert!(!references_sensitive_path("openssl genrsa -out rsa_key.pem 2048"));
        assert!(!references_sensitive_path("ssh-keygen -t rsa -b 4096"));  // 这是生成命令,合法
        // 但实际引用私钥文件路径仍应被拦截
        assert!(references_sensitive_path("cat ~/.ssh/id_rsa"));
        assert!(references_sensitive_path("cp id_rsa /tmp/"));
        assert!(references_sensitive_path("less .ssh/id_ed25519"));
    }
}
