//! 模型连接配置与本机模型凭据。调用方在发网络请求前取出一份配置，不跨请求持锁。
use crate::{error::invalid, http_json::Json, Error, Result};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::Path};

#[cfg(not(test))]
use crate::platform_credentials::{read_strict as read_key, remove_strict as remove_key, write as write_key};

const DEFAULT_BASE: &str = "https://api.deepseek.com";
const DEFAULT_MODEL: &str = "deepseek-chat";
const MIGRATION_PENDING: &str = "模型 API key 尚未迁移，请重新打开图库或保存模型设置";
const READ_FAILED: &str = "无法读取模型 API key，请重新填写或清除";

pub(super) struct Options {
    base: String,
    model: String,
    slot: Option<String>,
    legacy: Option<String>,
    has_legacy: bool,
}

impl Options {
    pub(super) fn read(c: &Connection) -> Result<Self> {
        let mut rows: HashMap<String, String> = c
            .prepare(
                "SELECT key,value FROM name_settings WHERE key IN (
                'model_base','model_name','deepseek_model','model_credential','model_key','deepseek_key')",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let current = rows.remove("model_key");
        let old = rows.remove("deepseek_key");
        Ok(Self {
            base: rows.remove("model_base").unwrap_or_else(|| DEFAULT_BASE.into()),
            model: rows
                .remove("model_name")
                .or_else(|| rows.remove("deepseek_model"))
                .unwrap_or_else(|| DEFAULT_MODEL.into()),
            slot: rows.remove("model_credential"),
            has_legacy: current.is_some() || old.is_some(),
            legacy: current.or(old).filter(|s| !s.is_empty()),
        })
    }

    pub(super) fn has_credential(&self) -> bool {
        self.has_legacy || self.slot.is_some()
    }

    pub(super) fn credentials(self, path: &Path) -> Result<Credentials> {
        if self.has_legacy {
            return invalid(MIGRATION_PENDING);
        }
        let key = self.key(path)?;
        Ok(Credentials { key, model: self.model, base: self.base })
    }

    fn key(&self, path: &Path) -> Result<Option<String>> {
        self.slot.as_ref().map(|slot| read_key(&account(path, &self.base, slot))).transpose().map(Option::flatten)
    }
}

/// 凭据绑定到图库包的位置、保存时的接口及这次保存的版本。同 ID 的副本和改过地址的设置都不能取到原凭据。
fn account(path: &Path, base: &str, slot: &str) -> String {
    // 按平台原生路径编码固定摘要输入，避免 Rust 的内部 OsStr 编码变化后找不到已有条目。
    #[cfg(unix)]
    let path_bytes = std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str());
    #[cfg(windows)]
    let encoded: Vec<u8> =
        std::os::windows::ffi::OsStrExt::encode_wide(path.as_os_str()).flat_map(u16::to_le_bytes).collect();
    #[cfg(windows)]
    let path_bytes = encoded.as_slice();
    let mut hash = Sha256::new();
    for bytes in [path_bytes, base.as_bytes(), slot.as_bytes()] {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    format!("model-api-{:x}", hash.finalize())
}

fn new_slot() -> Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| Error::invalid("无法生成模型凭据引用"))?;
    Ok(format!("{:032x}", u128::from_le_bytes(bytes)))
}

pub(super) struct Credentials {
    pub(super) key: Option<String>,
    pub(super) model: String,
    pub(super) base: String,
}

pub(crate) fn status(c: &Connection, path: &Path) -> Result<Json> {
    let options = Options::read(c)?;
    let (key, error) = if options.has_legacy {
        (None, Some(MIGRATION_PENDING))
    } else {
        match options.key(path) {
            Ok(key) => (key, None),
            // 设置卡仍须可用，才能替换损坏的凭据。界面只收到固定的脱敏错误。
            Err(_) => (None, Some(READ_FAILED)),
        }
    };
    let mut status = Json::object([
        ("configured", key.is_some().into()),
        ("hint", key.map(|s| s.chars().skip(s.chars().count().saturating_sub(4)).collect::<String>()).into()),
        ("model", options.model.into()),
        ("base", options.base.into()),
    ]);
    if let Some(error) = error {
        status.set("credential_error", error.into());
    }
    Ok(status)
}

/// 会话开始时只处理尚有明文的图库。失败时保留原设置，下次打开或保存时可重试。
pub(super) fn migrate(path: &Path) -> Result<()> {
    let c = crate::app_library::read_connection(path)?;
    let needs_migration: bool =
        c.query_row("SELECT EXISTS(SELECT 1 FROM name_settings WHERE key IN ('model_key','deepseek_key'))", [], |r| {
            r.get(0)
        })?;
    drop(c);
    if needs_migration {
        change(path, None, None, None)?;
    }
    Ok(())
}

pub(super) fn configure(path: &Path, body: &Json) -> Result<()> {
    let text = |k: &str| -> Result<Option<&str>> {
        match body.get(k) {
            None | Some(Json::Scalar(Value::Null)) => Ok(None),
            Some(Json::Scalar(Value::String(s))) => Ok(Some(s)),
            _ => invalid("模型设置格式不正确"),
        }
    };
    let key = text("key")?.map(str::trim);
    let model = text("model")?;
    let base = text("base")?;
    if let Some(k) = key {
        if !(5..=512).contains(&k.chars().count()) || k.chars().any(char::is_whitespace) {
            return invalid("API key 格式不正确");
        }
    }
    if let Some(m) = model {
        if !regex::Regex::new(r"^[\w.:/-]{1,100}$").expect("固定的正则表达式必须有效").is_match(m) {
            return invalid("模型名称格式不正确");
        }
    }
    if let Some(b) = base {
        let u =
            url::Url::parse(b.trim()).map_err(|_| Error::invalid("接口地址须为 https 地址（本机服务可用 http）"))?;
        if b.chars().count() > 300
            || u.host_str().is_none()
            || u.query().is_some()
            || u.fragment().is_some()
            || !(u.scheme() == "https"
                || (u.scheme() == "http" && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
        {
            return invalid("接口地址须为 https 地址（本机服务可用 http）");
        }
    }
    change(path, body.get("key").map(|_| key), model, base.map(|b| b.trim().trim_end_matches('/')))
}

/// `key` 外层 Option 区分省略和显式清除。改地址时只能使用和它一起提交的密钥。
fn change(path: &Path, key: Option<Option<&str>>, model: Option<&str>, base: Option<&str>) -> Result<()> {
    let mut c = super::net::db(path)?;
    c.pragma_update(None, "secure_delete", true)?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let previous = Options::read(&tx)?;
    let next_base = base.unwrap_or(&previous.base);
    let change_key = key.or_else(|| (next_base != previous.base).then_some(None));
    // 仅当系统存储确实已有凭据时才丢弃回退版本写下的旧密钥。读取失败则回滚，保留明文供重试。
    let install = match change_key {
        Some(key) => key,
        None if previous.legacy.is_some() && previous.key(path)?.is_none() => previous.legacy.as_deref(),
        None => None,
    };
    let slot = install.map(|_| new_slot()).transpose()?;

    tx.execute("DELETE FROM name_settings WHERE key IN ('model_key','deepseek_key')", [])?;
    if change_key.is_some() || slot.is_some() {
        tx.execute("DELETE FROM name_settings WHERE key='model_credential'", [])?;
    }
    for (k, v) in [("model_name", model), ("model_base", base), ("model_credential", slot.as_deref())] {
        if let Some(v) = v {
            tx.execute("INSERT OR REPLACE INTO name_settings VALUES (?,?)", [k, v])?;
        }
    }
    // 新条目写成功才发布它的引用。用新的引用写入，失败可以保留原凭据。
    let staged = slot.as_deref().zip(install).map(|(slot, value)| (account(path, next_base, slot), value));
    if let Some((account, value)) = &staged {
        write_key(account, value)?;
    }
    if let Err(error) = tx.commit() {
        if let Some((account, _)) = &staged {
            cleanup(account);
        }
        return Err(error.into());
    }

    if change_key.is_some() {
        if let Some(slot) = previous.slot {
            cleanup(&account(path, &previous.base, &slot));
        }
    }
    if previous.has_legacy {
        let busy: i64 = c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
        if busy != 0 {
            eprintln!("已迁移模型凭据，图库的 WAL 正在使用，暂不能截断");
        }
    }
    Ok(())
}

fn cleanup(account: &str) {
    if remove_key(account).is_err() {
        eprintln!("无法清理名称翻译的旧模型凭据");
    }
}

// 测试构建只用进程内合成凭据。真实平台存储由独立目录的宿主测试覆盖。
#[cfg(test)]
thread_local! {
    static KEYS: std::cell::RefCell<HashMap<String, String>> = Default::default();
}
#[cfg(test)]
fn read_key(kind: &str) -> Result<Option<String>> {
    Ok(KEYS.with_borrow(|keys| keys.get(kind).cloned()))
}
#[cfg(test)]
fn write_key(kind: &str, key: &str) -> Result<()> {
    KEYS.with_borrow_mut(|keys| keys.insert(kind.to_owned(), key.to_owned()));
    Ok(())
}
#[cfg(test)]
fn remove_key(kind: &str) -> Result<()> {
    KEYS.with_borrow_mut(|keys| keys.remove(kind));
    Ok(())
}
