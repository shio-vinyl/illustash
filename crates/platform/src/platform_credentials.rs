//! 凭据存储的平台边界；账号校验和授权仍在 app_accounts 里。
use crate::Result;
#[cfg(windows)]
#[path = "app_dpapi.rs"]
mod dpapi;
#[cfg(test)]
use std::process::Command;
use std::{fs, path::PathBuf};
#[cfg(target_os = "macos")]
#[path = "platform_credentials_macos.rs"]
mod macos;
pub fn support() -> Result<PathBuf> {
    // 显式配置时不要求能定位主目录，与原来的求值顺序一致。
    if let Some(configured) = std::env::var_os("ILLUSTASH_APP_SUPPORT") {
        return Ok(PathBuf::from(configured));
    }
    Ok(crate::app_paths::app_support(&crate::app_paths::user_home()?))
}
fn keychain() -> bool {
    cfg!(target_os = "macos") && std::env::var("ILLUSTASH_NO_KEYCHAIN").as_deref() != Ok("1")
}
pub fn read(kind: &str) -> Result<Option<String>> {
    let value = if keychain() {
        #[cfg(target_os = "macos")]
        {
            String::from_utf8_lossy(&macos::Store::default().read(kind).unwrap_or_default()).into_owned()
        }
        #[cfg(not(target_os = "macos"))]
        {
            unreachable!("macOS only")
        }
    } else if cfg!(windows) {
        windows_secret(kind)?.unwrap_or_default()
    } else {
        fs::read_to_string(support()?.join("accounts").join(kind)).unwrap_or_default()
    };
    Ok((!value.trim().is_empty()).then(|| value.trim().to_string()))
}
/// 读取凭据；只有条目确实不存在时才返回空。适用于需要分辨存储故障与未配置的调用方。
pub fn read_strict(kind: &str) -> Result<Option<String>> {
    #[cfg(target_os = "macos")]
    if keychain() {
        return match macos::Store::default().read(kind) {
            Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| crate::Error::invalid("钥匙串凭据格式无效")),
            Err(macos::NOT_FOUND) => Ok(None),
            Err(_) => Err(crate::Error::invalid("读取钥匙串失败")),
        };
    }
    #[cfg(windows)]
    return windows_secret(kind);
    #[cfg(not(windows))]
    match fs::read_to_string(secret_path(kind)?) {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub fn write(kind: &str, value: &str) -> Result<()> {
    if keychain() {
        #[cfg(target_os = "macos")]
        macos::Store::default().write(kind, value.as_bytes()).map_err(|_| crate::Error::invalid("写入钥匙串失败"))?;
        #[cfg(not(target_os = "macos"))]
        unreachable!("macOS only");
    } else if cfg!(windows) {
        windows_put(kind, value)?;
    } else {
        let path = support()?.join("accounts").join(kind);
        fs::create_dir_all(path.parent().expect("internal file path must have a parent"))?;
        use std::io::Write;
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        crate::platform_filesystem::private_file_options(&mut opts);
        let mut f = opts.open(&path)?;
        crate::platform_filesystem::set_private_file_permissions(&f)?;
        f.write_all(value.as_bytes())?;
    }
    Ok(())
}
pub fn remove(kind: &str) -> Result<()> {
    if keychain() {
        #[cfg(target_os = "macos")]
        let _ = macos::Store::default().remove(kind);
        #[cfg(not(target_os = "macos"))]
        unreachable!("macOS only");
        Ok(())
    } else {
        remove_file(kind)
    }
}
/// 删除凭据，条目不存在时成功，其他存储错误由调用方处理。
pub fn remove_strict(kind: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    if keychain() {
        return macos::Store::default().remove(kind).map_err(|_| crate::Error::invalid("删除钥匙串凭据失败"));
    }
    remove_file(kind)
}
fn remove_file(kind: &str) -> Result<()> {
    match fs::remove_file(secret_path(kind)?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
// Windows 从不读取或迁移旧的明文文件。重新绑定时另写一个 DPAPI blob；
// 卸载或回退时不会把它误当成明文。
fn secret_path(kind: &str) -> Result<PathBuf> {
    Ok(support()?.join("accounts").join(if cfg!(windows) { format!("{kind}.dpapi") } else { kind.into() }))
}
#[cfg(windows)]
fn windows_secret(kind: &str) -> Result<Option<String>> {
    let bytes = match fs::read(secret_path(kind)?) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    String::from_utf8(dpapi::unprotect(&bytes, kind)?)
        .map(Some)
        .map_err(|_| crate::Error::invalid("Windows 凭据格式无效，请重新绑定"))
}
#[cfg(windows)]
fn windows_put(kind: &str, value: &str) -> Result<()> {
    let bytes = dpapi::protect(value.as_bytes(), kind)?;
    let path = secret_path(kind)?;
    fs::create_dir_all(path.parent().expect("credential path parent"))?;
    // 账号操作是串行的。任何时候只写密文，临时文件也一样；加密失败时不会覆盖旧的 blob。
    let tmp = path.with_extension("dpapi.tmp");
    {
        use std::io::Write;
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    fs::rename(tmp, path)?;
    Ok(())
}
#[cfg(not(windows))]
fn windows_secret(_: &str) -> Result<Option<String>> {
    unreachable!("Windows only")
}
#[cfg(not(windows))]
fn windows_put(_: &str, _: &str) -> Result<()> {
    unreachable!("Windows only")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn synthetic_storage_override_without_home() {
        const CHILD: &str = "ILLUSTASH_SYNTHETIC_CREDENTIAL_TEST";
        if std::env::var(CHILD).as_deref() == Ok("1") {
            let path = support().unwrap();
            assert!(std::env::var_os("HOME").is_none());
            assert!(std::env::var_os("USERPROFILE").is_none());
            assert_eq!(read("saucenao").unwrap(), None);
            write("saucenao", "  synthetic-only-合成凭据  ").unwrap();
            assert_eq!(read("saucenao").unwrap().as_deref(), Some("synthetic-only-合成凭据"));
            write("saucenao", "replacement-synthetic").unwrap();
            assert_eq!(read("saucenao").unwrap().as_deref(), Some("replacement-synthetic"));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(fs::metadata(path.join("accounts/saucenao")).unwrap().permissions().mode() & 0o777, 0o600);
            }
            remove("saucenao").unwrap();
            remove("saucenao").unwrap();
            assert_eq!(read("saucenao").unwrap(), None);
            fs::remove_dir_all(path).unwrap();
            return;
        }
        // 环境变量的改动与其他所有测试和真实存储隔离开。
        let path = std::env::temp_dir().join(format!("illustash-117-credentials-{}", std::process::id()));
        assert!(!path.exists());
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "platform_credentials::tests::synthetic_storage_override_without_home", "--nocapture"])
            .env(CHILD, "1")
            .env("ILLUSTASH_APP_SUPPORT", &path)
            .env("ILLUSTASH_NO_KEYCHAIN", "1")
            .env_remove("HOME")
            .env_remove("USERPROFILE")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        assert!(!path.exists());
    }
}
