//! 敏感路径检测(SensitivePathGate)。
//!
//! 设计参考:atomcode `sensitive_path.rs` 与 claudecode `.ssh/.aws/.env` 黑名单。
//! 当 bash 命令字符串中包含这些敏感路径标记时,直接 fail-closed。

/// 敏感路径标记(子串匹配,大小写不敏感)
const SENSITIVE_MARKERS: &[&str] = &[
    // ───── SSH 私钥 ─────
    "/.ssh/",
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
    "id_xmss",
    // ───── 云凭证 ─────
    "/.aws/",
    "/.aws/credentials",
    "/.azure/",
    "/.gcp/",
    "/.config/gcloud/",
    "/.kube/",
    // ───── GPG / 加密密钥 ─────
    "/.gnupg/",
    // ───── Token / .netrc ─────
    ".netrc",
    "_netrc",
    ".git-credentials",
    ".gitconfig",
    ".npmrc",
    ".pypirc",
    // ───── 证书 / keystore ─────
    ".pem",
    ".p12",
    ".pfx",
    ".keystore",
    ".jks",
    // ───── Terraform / Vault ─────
    "/.terraform.d/",
    ".terraformrc",
    // ───── Docker / CI ─────
    ".docker/config.json",
    "/run/secrets/",
];

/// 命令字符串是否引用了敏感路径。
///
/// 检测时大小写不敏感,quoted string 内的也算(因为最终 shell 会展开)。
pub fn references_sensitive_path(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    for marker in SENSITIVE_MARKERS {
        if lower.contains(marker) {
            return true;
        }
    }
    env_file_sensitive(&lower)
        || shell_history_sensitive(&lower)
        || proc_self_environ(&lower)
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
        assert!(references_sensitive_path("cat cert.pem"));
        assert!(references_sensitive_path("keytool -importkeystore src.p12"));
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
}
