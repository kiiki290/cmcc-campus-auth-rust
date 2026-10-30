//! 密码存储：交给操作系统密钥环（Windows 凭据管理器 / Linux Secret Service）。
//!
//! 配置文件只保存学号等非敏感信息，密码不落盘（密钥环内部由系统加密）。

use anyhow::Result;

const SERVICE: &str = "cmcc-campus-auth";
const ACCOUNT: &str = "password";

fn entry() -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(SERVICE, ACCOUNT)?)
}

pub fn save_password(password: &str) -> Result<()> {
    entry()?.set_password(password)?;
    Ok(())
}

pub fn load_password() -> Result<Option<String>> {
    match entry()?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

pub fn delete_password() -> Result<()> {
    match entry()?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "需要桌面密钥环（Linux Secret Service / Windows 凭据管理器）"]
    fn keyring_roundtrip() {
        let entry = keyring::Entry::new("cmcc-campus-auth-test", "roundtrip").unwrap();
        entry.set_password("测试密码-123").unwrap();
        assert_eq!(entry.get_password().unwrap(), "测试密码-123");
        entry.delete_credential().unwrap();
        assert!(matches!(entry.get_password(), Err(keyring::Error::NoEntry)));
    }
}
