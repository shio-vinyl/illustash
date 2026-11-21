pub use illustash_platform::sqlite::*;

/// 在任何读取或执行归档里的 schema 之前，配置这一条 SQLite 连接。
pub(crate) fn protect_archive(c: &rusqlite::Connection) -> rusqlite::Result<()> {
    use rusqlite::config::DbConfig;
    c.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    c.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::OpenFlags;
    use std::fs;

    #[test]
    fn archive_flags_apply_to_the_supplied_connection() {
        use rusqlite::config::DbConfig;

        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let other = rusqlite::Connection::open_in_memory().unwrap();
        connection.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, true).unwrap();
        connection.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, false).unwrap();
        other.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, true).unwrap();
        other.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, false).unwrap();
        protect_archive(&connection).unwrap();
        assert!(!connection.db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA).unwrap());
        assert!(connection.db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE).unwrap());
        assert!(other.db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA).unwrap());
        assert!(!other.db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE).unwrap());
    }

    #[test]
    fn long_path_create_attach_wal_backup_and_restore() {
        let root = std::env::temp_dir().join(format!("illustash-long-create-{}", crate::store::new_key().unwrap()));
        let mut parent = root.clone();
        while parent.as_os_str().len() < 200 {
            parent = parent.join("synthetic-segment");
        }
        fs::create_dir_all(&parent).unwrap();
        let reference = parent.join("参考 # %.sqlite");
        open(&reference).unwrap().execute_batch(include_str!("app_source_empty.sql")).unwrap();
        let dest = parent.join("synthetic.pilib");
        crate::app_create::create_folder(&dest, &parent, Some("synthetic"), Some("other"), Some(&reference)).unwrap();
        let lib = crate::app_library::Library::open(&dest).unwrap();
        let writer = open(lib.db()).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE long_path_probe(value TEXT); INSERT INTO long_path_probe VALUES ('committed-wal');").unwrap();
        let backup = crate::app_settings::backup(&lib).unwrap();
        #[cfg(windows)]
        assert!(backup.as_os_str().len() > 260);
        let restored = parent.join("restored.sqlite");
        fs::copy(&backup, &restored).unwrap();
        let mut c = open_with_flags(&restored, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            c.query_row("SELECT value FROM long_path_probe", [], |r| r.get::<_, String>(0)).unwrap(),
            "committed-wal"
        );
        crate::app_library::check_database(&mut c).unwrap();
        println!(
            "synthetic library path units: {}; backup path units: {}",
            dest.as_os_str().len(),
            backup.as_os_str().len()
        );
        drop((c, writer));
        fs::remove_dir_all(root).unwrap();
    }
}
