//! Обслуживание SQLite-файлов (вкладка «Хранилище»): сжатие и бэкап.
//! Работает по ОТДЕЛЬНОМУ соединению (writer'ы живут своими): WAL это допускает,
//! эксклюзив VACUUM берётся между батчами writer'а (busy_timeout ждёт).

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

/// Checkpoint WAL (TRUNCATE) + VACUUM: возвращает диску место удалённых строк и
/// сбрасывает разросшийся -wal. Блокирует писателей на время VACUUM — операция
/// по явному действию пользователя.
pub fn compact_db(path: &Path) -> anyhow::Result<()> {
    let conn = Connection::open(path)?;
    let _ = conn.busy_timeout(Duration::from_secs(30));
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    conn.execute("VACUUM", [])?;
    Ok(())
}

/// Консистентный бэкап одним файлом (`VACUUM INTO`): работает на живой БД, не
/// останавливая writer. Целевой файл не должен существовать.
pub fn backup_db(src: &Path, dst: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        !dst.exists(),
        "файл бэкапа уже существует: {}",
        dst.display()
    );
    let conn = Connection::open(src)?;
    let _ = conn.busy_timeout(Duration::from_secs(30));
    let dst_sql = dst.to_string_lossy().replace('\'', "''");
    conn.execute(&format!("VACUUM INTO '{dst_sql}'"), [])?;
    Ok(())
}

/// Размер файла БД и его -wal (байты); отсутствующие файлы = 0.
pub fn db_sizes(path: &Path) -> (u64, u64) {
    let main = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    let wal = std::fs::metadata(std::path::PathBuf::from(wal))
        .map(|m| m.len())
        .unwrap_or(0);
    (main, wal)
}
