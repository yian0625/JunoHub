//! Consistent SQLite online snapshot. Never copies live WAL files or prints configuration.
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::{
    error::Error,
    path::{Path, PathBuf},
};

async fn snapshot(pool: &SqlitePool, destination: &Path) -> Result<(), Box<dyn Error>> {
    if !destination.is_absolute() || destination.exists() {
        return Err("destination must be a new absolute path".into());
    }
    let directory = destination.parent().ok_or("missing parent directory")?;
    std::fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let temporary = directory.join(format!(".{}.part", uuid::Uuid::new_v4()));
    sqlx::query("VACUUM INTO ?")
        .bind(temporary.to_str().ok_or("invalid destination")?)
        .execute(pool)
        .await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    let check = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}?mode=ro", temporary.display()))
        .await?;
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&check)
        .await?;
    check.close().await;
    if integrity != "ok" {
        return Err("snapshot failed integrity check".into());
    }
    // A hard link publishes without replacing an existing backup.
    std::fs::hard_link(&temporary, destination)?;
    std::fs::remove_file(&temporary)?;
    Ok(())
}
#[tokio::main]
async fn main() {
    let Some(destination) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("Usage: junohub-backup /data/backups/new-name.sqlite");
        std::process::exit(2);
    };
    let url = std::env::var("JUNOHUB_DATABASE_URL")
        .unwrap_or_else(|_| "sqlite:///data/junohub.db?mode=rw".into());
    let result = async {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await?;
        let result = snapshot(&pool, &destination).await;
        pool.close().await;
        result
    }
    .await;
    if result.is_err() {
        eprintln!("SQLite 备份失败；请检查数据库、目标路径和磁盘空间。未输出配置或凭据。");
        std::process::exit(1);
    }
    println!("SQLite 备份已完成并通过完整性校验。");
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn snapshot_is_consistent_and_never_overwrites_existing_backups() {
        let directory =
            std::env::temp_dir().join(format!("junohub-backup-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("snapshot.sqlite");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!(
                "sqlite://{}?mode=rwc",
                directory.join("source.sqlite").display()
            ))
            .await
            .unwrap();
        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE sample(value TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO sample VALUES('before')")
            .execute(&pool)
            .await
            .unwrap();
        snapshot(&pool, &path).await.unwrap();
        sqlx::query("UPDATE sample SET value='after'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(snapshot(&pool, &path).await.is_err());
        let restored = SqlitePoolOptions::new()
            .connect(&format!("sqlite://{}?mode=ro", path.display()))
            .await
            .unwrap();
        let value: String = sqlx::query_scalar("SELECT value FROM sample")
            .fetch_one(&restored)
            .await
            .unwrap();
        assert_eq!(value, "before");
        restored.close().await;
        pool.close().await;
        std::fs::remove_dir_all(directory).unwrap();
    }
}
