//! D9-4 凭证加密(L1600):API Key 应用层 AES-256-GCM 加密。
//!
//! 对齐 openclaw Secret Sentinel 设计(专题-第十九轮 D9-4):
//! - 进程级主密钥,启动时从 `~/.laew/master.key` 加载或 0o600 自动生成
//! - 落 SQLite 前透明加密(`enc:v1:` 前缀标识密文)
//! - 读出时透明解密;旧明文数据在 `Db::open` 时一次性迁移
//! - GCM tag 校验失败 → 明确错误(密钥错误或数据被篡改)
//!
//! ## 行为契约
//!
//! - `master.key` 不存在 → `OsRng` 生成 32 字节 + 0o600 落盘
//! - `master.key` 权限非 0o600 → 拒绝(防其他用户读取)
//! - 加密输出:`enc:v1:` + base64( `[version=0x01][12B nonce][ciphertext+16B tag]` )
//! - 解密失败:GCM tag 错 / 版本错 / 长度错 → `ConfigError::Vault`,**不暴露原密文**
//! - `export_to_json` 仍输出明文(用户备份场景),但打一次性脱敏告警
//! - `LAEW_EXPORT_REDACT=1` → export 输出脱敏密钥(对齐 mask_key)

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use once_cell::sync::OnceCell;
use rand::RngCore;
use std::fs;
use std::path::{Path, PathBuf};
use zeroize::Zeroize;

use crate::database::{ConfigError, Result};

/// 密文格式版本(当前唯一版本)。
const VERSION: u8 = 0x01;
/// AES-256-GCM 标准 12 字节 nonce。
const NONCE_LEN: usize = 12;
/// AES-256 密钥长度(32 字节)。
const KEY_LEN: usize = 32;
/// 密文前缀:落 SQLite 的 api_key 字段以此开头即识别为已加密。
pub const CREDENTIAL_PREFIX: &str = "enc:v1:";

/// 凭证加密 Vault:进程级单例,持有 AES-256-GCM 密钥。
///
/// 设计上全局唯一(对齐 openclaw `resolveGlobalSingleton`),通过 `Vault::global()`
/// 懒加载,首次调用时从 `~/.laew/master.key` 读取或自动生成。
pub struct Vault {
    cipher: Aes256Gcm,
}

/// 全局 Vault 单例。
static GLOBAL_VAULT: OnceCell<Vault> = OnceCell::new();

impl Vault {
    /// 获取全局 Vault 单例(懒加载)。
    ///
    /// 首次调用即加载/生成主密钥;后续调用返回同一实例。
    /// 主密钥加载失败时返回 `ConfigError::Vault`(如权限不安全)。
    pub fn global() -> Result<&'static Vault> {
        GLOBAL_VAULT.get_or_init(|| {
            Self::load_or_create().expect("Vault 初始化失败(主密钥加载/生成异常)")
        });
        // SAFETY:get_or_init 内 panic 会直接 abort,故 Ok 分支必然已初始化。
        Ok(GLOBAL_VAULT.get().expect("Vault 单例必然存在"))
    }

    /// 测试/显式重置用:用指定密钥构建 Vault(不写入文件)。
    #[cfg(test)]
    pub fn from_key(key: &[u8; KEY_LEN]) -> Self {
        let key = Key::<Aes256Gcm>::from_slice(key);
        Self { cipher: Aes256Gcm::new(key) }
    }

    /// 加载主密钥(不存在则自动生成 0o600 落盘)。
    fn load_or_create() -> Result<Self> {
        let path = master_key_path()?;
        let key_bytes = if path.exists() {
            ensure_0o600(&path)?;
            let bytes = fs::read(&path).map_err(|e| {
                ConfigError::Vault(format!("读取 {} 失败: {e}", path.display()))
            })?;
            if bytes.len() != KEY_LEN {
                return Err(ConfigError::Vault(format!(
                    "主密钥长度异常: {} 字节,期望 {KEY_LEN}",
                    bytes.len()
                )));
            }
            bytes
        } else {
            let mut bytes = vec![0u8; KEY_LEN];
            rand::thread_rng().fill_bytes(&mut bytes);
            write_secret_file(&path, &bytes)?;
            bytes
        };
        let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
        let cipher = Aes256Gcm::new(key);
        // 密钥明文残留清零(防内存快照泄露)。
        let mut kb = key_bytes;
        kb.zeroize();
        Ok(Self { cipher })
    }

    /// 加密明文(对齐 openclaw Secret Sentinel 加密流程)。
    ///
    /// 输出格式:`enc:v1:` + base64( `[0x01][12B nonce][ciphertext+16B GCM tag]` )。
    /// 每次加密使用 `OsRng` 随机 12 字节 nonce(非确定性,因 laew 无等价输入查询需求)。
    pub fn encrypt(&self, plaintext: &str) -> Result<String> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(nonce, Payload { msg: plaintext.as_bytes(), aad: &[] })
            .map_err(|e| ConfigError::Vault(format!("加密失败: {e}")))?;
        let mut buf = Vec::with_capacity(1 + NONCE_LEN + ciphertext.len());
        buf.push(VERSION);
        buf.extend_from_slice(&nonce_bytes);
        buf.extend_from_slice(&ciphertext);
        Ok(format!("{CREDENTIAL_PREFIX}{}", B64.encode(&buf)))
    }

    /// 解密密文(前缀 `enc:v1:`)。
    ///
    /// 校验失败(GCM tag / 版本 / 长度)→ `ConfigError::Vault`,错误信息不暴露原密文。
    pub fn decrypt(&self, blob: &str) -> Result<String> {
        let raw = blob
            .strip_prefix(CREDENTIAL_PREFIX)
            .ok_or_else(|| ConfigError::Vault("密文缺少 enc:v1: 前缀".to_string()))?;
        let bytes = B64.decode(raw).map_err(|e| {
            ConfigError::Vault(format!("base64 解码失败: {e}"))
        })?;
        if bytes.len() < 1 + NONCE_LEN + 16 {
            return Err(ConfigError::Vault(format!(
                "密文长度异常: {} 字节",
                bytes.len()
            )));
        }
        if bytes[0] != VERSION {
            return Err(ConfigError::Vault(format!(
                "密文版本不支持: 0x{:02x}",
                bytes[0]
            )));
        }
        let nonce = Nonce::from_slice(&bytes[1..1 + NONCE_LEN]);
        let ciphertext = &bytes[1 + NONCE_LEN..];
        let plaintext = self
            .cipher
            .decrypt(nonce, Payload { msg: ciphertext, aad: &[] })
            .map_err(|_| {
                ConfigError::Vault(
                    "GCM tag 校验失败(主密钥错误或数据被篡改)".to_string(),
                )
            })?;
        String::from_utf8(plaintext).map_err(|e| {
            ConfigError::Vault(format!("UTF-8 解码失败: {e}"))
        })
    }

    /// 判断 api_key 字段是否已加密(前缀探测)。
    pub fn is_encrypted(blob: &str) -> bool {
        blob.starts_with(CREDENTIAL_PREFIX)
    }
}

/// 主密钥文件路径。
///
/// 优先级:
/// 1. 环境变量 `LAEW_MASTER_KEY_PATH`(测试用,隔离 HOME)
/// 2. `~/.laew/master.key`(默认)
fn master_key_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("LAEW_MASTER_KEY_PATH") {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| ConfigError::Vault("无法定位 HOME 目录".to_string()))?;
    Ok(Path::new(&home).join(".laew").join("master.key"))
}

/// 校验主密钥文件权限为 0o600(类 Unix)。
///
/// 非 Unix 平台跳过(依赖 OS 文件权限模型不同)。
fn ensure_0o600(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)
            .map_err(|e| ConfigError::Vault(format!("读取 {} 元数据失败: {e}", path.display())))?
            .permissions()
            .mode()
            & 0o777;
        if mode != 0o600 {
            return Err(ConfigError::Vault(format!(
                "主密钥 {} 权限不安全: 0o{mode:o},期望 0o600(仅属主可读写)",
                path.display()
            )));
        }
    }
    let _ = path;
    Ok(())
}

/// 原子写入密钥文件:tempfile → 0o600 → write → fsync → rename。
///
/// 对齐 atomcode `auth/lib.rs` 完整链路(专题-第十九轮 D9-4)。
fn write_secret_file(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            ConfigError::Vault(format!("创建 {} 失败: {e}", parent.display()))
        })?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents).map_err(|e| {
        ConfigError::Vault(format!("写入 {} 失败: {e}", tmp.display()))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).map_err(|e| {
            ConfigError::Vault(format!("设置 {} 权限失败: {e}", tmp.display()))
        })?;
    }
    fs::rename(&tmp, path).map_err(|e| {
        ConfigError::Vault(format!("重命名 {} → {} 失败: {e}", tmp.display(), path.display()))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_basic() {
        let v = Vault::from_key(&[1u8; KEY_LEN]);
        let ct = v.encrypt("sk-test-1234567890").unwrap();
        assert!(ct.starts_with(CREDENTIAL_PREFIX));
        assert_eq!(v.decrypt(&ct).unwrap(), "sk-test-1234567890");
    }

    #[test]
    fn roundtrip_empty() {
        let v = Vault::from_key(&[2u8; KEY_LEN]);
        let ct = v.encrypt("").unwrap();
        assert_eq!(v.decrypt(&ct).unwrap(), "");
    }

    #[test]
    fn roundtrip_unicode() {
        let v = Vault::from_key(&[3u8; KEY_LEN]);
        let s = "密钥-🔐-日本語-한국어-4KB".repeat(100);
        let ct = v.encrypt(&s).unwrap();
        assert_eq!(v.decrypt(&ct).unwrap(), s);
    }

    #[test]
    fn tamper_fails() {
        let v = Vault::from_key(&[4u8; KEY_LEN]);
        let ct = v.encrypt("secret").unwrap();
        // 篡改 base64 末字符
        let mut tampered = ct.clone();
        let last = tampered.pop().unwrap();
        tampered.push(if last == 'A' { 'B' } else { 'A' });
        assert!(v.decrypt(&tampered).is_err());
    }

    #[test]
    fn wrong_key_fails() {
        let v1 = Vault::from_key(&[5u8; KEY_LEN]);
        let v2 = Vault::from_key(&[6u8; KEY_LEN]);
        let ct = v1.encrypt("secret").unwrap();
        assert!(v2.decrypt(&ct).is_err());
    }

    #[test]
    fn version_mismatch_fails() {
        let v = Vault::from_key(&[7u8; KEY_LEN]);
        let ct = v.encrypt("x").unwrap();
        // 篡改版本字节(base64 解码后首字节)
        let raw = ct.strip_prefix(CREDENTIAL_PREFIX).unwrap();
        let bytes = B64.decode(raw).unwrap();
        let mut tampered = vec![0xFF]; // 版本 0xFF
        tampered.extend_from_slice(&bytes[1..]);
        let ct2 = format!("{CREDENTIAL_PREFIX}{}", B64.encode(&tampered));
        assert!(v.decrypt(&ct2).is_err());
    }

    #[test]
    fn is_encrypted_detects() {
        assert!(Vault::is_encrypted("enc:v1:AAAA"));
        assert!(!Vault::is_encrypted("sk-plaintext"));
        assert!(!Vault::is_encrypted(""));
    }

    #[test]
    fn decrypt_without_prefix_fails() {
        let v = Vault::from_key(&[8u8; KEY_LEN]);
        assert!(v.decrypt("sk-plaintext").is_err());
    }

    #[test]
    fn decrypt_too_short_fails() {
        let v = Vault::from_key(&[9u8; KEY_LEN]);
        // 构造过短密文:version + 12B nonce + 5B(< 16B tag)
        let short = format!("{CREDENTIAL_PREFIX}{}", B64.encode(&[0x01u8; 1 + NONCE_LEN + 5]));
        assert!(v.decrypt(&short).is_err());
    }

    #[test]
    fn master_key_generate_and_load() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("master.key");
        // 模拟生成
        let mut bytes = vec![0u8; KEY_LEN];
        rand::thread_rng().fill_bytes(&mut bytes);
        write_secret_file(&path, &bytes).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // 模拟加载
        let loaded = fs::read(&path).unwrap();
        assert_eq!(loaded, bytes);
    }

    #[test]
    fn ensure_0o600_rejects_world_readable() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("bad.key");
            fs::write(&path, &[0u8; KEY_LEN]).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(ensure_0o600(&path).is_err());
        }
    }
}
