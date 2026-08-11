use crate::errors::{AppError, AppResult};
use crate::provider::{GENERIC_PROVIDER_ID, MANAGED_PROVIDER_IDS};
use crate::security::hashes::sha256_file;
use crate::windows::atomic;
use chrono::Utc;
use rusqlite::backup::Backup;
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const SESSION_TABLES: [&str; 3] = ["threads", "sessions", "archived_sessions"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFileRecord {
    pub relative_path: String,
    pub before_sha256: String,
    pub after_sha256: String,
    pub original_provider: String,
    pub new_provider: String,
    pub modified_records: usize,
    pub migrated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBackupRecord {
    pub relative_path: String,
    pub backup_relative_path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Default)]
pub struct MigrationSummary {
    pub records: Vec<SessionFileRecord>,
    pub total_changes: usize,
}

fn collect_jsonl(directory: &Path, output: &mut Vec<PathBuf>) -> AppResult<()> {
    if !directory.is_dir() {
        return Ok(());
    }
    let mut stack = vec![directory.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current)
            .map_err(|error| AppError::io("SESSION-001", "无法读取会话目录", &error))?
        {
            let entry =
                entry.map_err(|error| AppError::io("SESSION-002", "无法检查会话文件", &error))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|error| AppError::io("SESSION-003", "无法读取会话类型", &error))?;
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file()
                && path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("jsonl"))
            {
                output.push(path);
            }
        }
    }
    Ok(())
}

pub fn candidate_files(home: &Path) -> AppResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_jsonl(&home.join("sessions"), &mut files)?;
    collect_jsonl(&home.join("archived_sessions"), &mut files)?;
    let database = home.join("state_5.sqlite");
    if database.is_file() {
        files.push(database);
    }
    files.sort();
    Ok(files)
}

pub fn estimated_size_for(files: &[PathBuf]) -> AppResult<u64> {
    files.iter().try_fold(0_u64, |total, path| {
        fs::metadata(path)
            .map(|metadata| total.saturating_add(metadata.len()))
            .map_err(|error| AppError::io("SESSION-004", "无法估算会话备份大小", &error))
    })
}

fn relative_string(home: &Path, path: &Path) -> AppResult<String> {
    path.strip_prefix(home)
        .map(|value| value.to_string_lossy().replace('\\', "/"))
        .map_err(|_| AppError::new("SESSION-005", "会话文件不在 Codex 配置目录内。"))
}

fn backup_path(backup_root: &Path, relative: &str) -> PathBuf {
    backup_root.join("data").join(relative.replace('/', "\\"))
}

fn backup_sqlite(source: &Path, destination: &Path) -> AppResult<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| AppError::io("SESSION-006", "无法创建数据库备份目录", &error))?;
    }
    if destination.exists() {
        fs::remove_file(destination)
            .map_err(|error| AppError::io("SESSION-007", "无法替换数据库备份", &error))?;
    }
    let source_connection = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| {
        AppError::new("SESSION-008", format!("无法读取会话数据库：{error}"))
    })?;
    let mut destination_connection = Connection::open(destination).map_err(|error| {
        AppError::new("SESSION-009", format!("无法创建会话数据库备份：{error}"))
    })?;
    let backup = Backup::new(&source_connection, &mut destination_connection)
        .map_err(|error| AppError::new("SESSION-010", format!("无法启动数据库备份：{error}")))?;
    backup
        .run_to_completion(64, Duration::from_millis(5), None)
        .map_err(|error| AppError::new("SESSION-011", format!("会话数据库备份失败：{error}")))?;
    Ok(())
}

fn copy_with_hash(source: &Path, destination: &Path) -> AppResult<String> {
    let mut input = File::open(source)
        .map_err(|error| AppError::io("SESSION-060", "无法读取待备份会话文件", &error))?;
    let output = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(destination)
        .map_err(|error| AppError::io("SESSION-061", "无法创建会话备份文件", &error))?;
    let mut output = BufWriter::with_capacity(1024 * 1024, output);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| AppError::io("SESSION-062", "无法读取会话备份数据", &error))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| AppError::io("SESSION-063", "无法写入会话备份数据", &error))?;
        hasher.update(&buffer[..read]);
    }
    output
        .flush()
        .map_err(|error| AppError::io("SESSION-064", "无法刷新会话备份文件", &error))?;
    Ok(format!("{:X}", hasher.finalize()))
}

pub fn backup_files(
    home: &Path,
    backup_root: &Path,
    files: &[PathBuf],
) -> AppResult<Vec<SessionBackupRecord>> {
    let mut records = Vec::new();
    for source in files {
        let relative = relative_string(home, &source)?;
        let destination = backup_path(backup_root, &relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| AppError::io("SESSION-012", "无法创建会话备份目录", &error))?;
        }
        let sha256 = if source
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("state_5.sqlite"))
        {
            backup_sqlite(source, &destination)?;
            sha256_file(&destination)?
        } else {
            copy_with_hash(source, &destination)?
        };
        let metadata = fs::metadata(&destination)
            .map_err(|error| AppError::io("SESSION-014", "无法校验会话备份", &error))?;
        records.push(SessionBackupRecord {
            relative_path: relative.clone(),
            backup_relative_path: format!("data/{relative}"),
            sha256,
            size: metadata.len(),
        });
    }
    Ok(records)
}

#[cfg(test)]
pub fn backup_all(home: &Path, backup_root: &Path) -> AppResult<Vec<SessionBackupRecord>> {
    backup_files(home, backup_root, &candidate_files(home)?)
}

pub fn restore_full_backup(
    home: &Path,
    backup_root: &Path,
    records: &[SessionBackupRecord],
) -> AppResult<()> {
    for record in records {
        let source = backup_root.join(record.backup_relative_path.replace('/', "\\"));
        let destination = home.join(record.relative_path.replace('/', "\\"));
        if sha256_file(&source)? != record.sha256 {
            return Err(AppError::new(
                "SESSION-015",
                format!("会话备份哈希不一致：{}", record.relative_path),
            ));
        }
        atomic::copy_atomic(&source, &destination)?;
    }
    Ok(())
}

fn line_ending(line: &str) -> (&str, &str) {
    if let Some(value) = line.strip_suffix("\r\n") {
        (value, "\r\n")
    } else if let Some(value) = line.strip_suffix('\n') {
        (value, "\n")
    } else {
        (line, "")
    }
}

fn session_meta(value: &Value) -> Option<(&str, &Value)> {
    if value.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    let payload = value.get("payload")?.as_object()?;
    let id = payload.get("id")?.as_str()?;
    let provider = payload.get("model_provider")?;
    Some((id, provider))
}

fn set_session_provider(value: &mut Value, provider: Value) -> bool {
    let Some(payload) = value.get_mut("payload").and_then(Value::as_object_mut) else {
        return false;
    };
    let Some(current) = payload.get_mut("model_provider") else {
        return false;
    };
    if *current == provider {
        return false;
    }
    *current = provider;
    true
}

fn provider_label(providers: &BTreeSet<String>) -> String {
    match providers.len() {
        0 => "none".to_string(),
        1 => providers.iter().next().cloned().unwrap_or_default(),
        _ => "mixed".to_string(),
    }
}

fn migrate_jsonl(home: &Path, path: &Path, target: &str) -> AppResult<Option<SessionFileRecord>> {
    let before_hash = sha256_file(path)?;
    let temporary = atomic::temporary_sibling(path)?;
    let input = File::open(path)
        .map_err(|error| AppError::io("SESSION-016", "无法读取会话 JSONL", &error))?;
    let output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| AppError::io("SESSION-017", "无法创建会话临时文件", &error))?;
    let mut writer = BufWriter::new(output);
    let mut modified = 0_usize;
    let mut originals = BTreeSet::new();

    let result = (|| {
        let mut reader = BufReader::new(input);
        let mut line = String::new();
        let mut line_index = 0_usize;
        loop {
            line.clear();
            let read = reader
                .read_line(&mut line)
                .map_err(|error| AppError::io("SESSION-018", "无法读取会话 JSONL 行", &error))?;
            if read == 0 {
                break;
            }
            line_index += 1;
            let (content, ending) = line_ending(&line);
            let mut value: Value = serde_json::from_str(content).map_err(|error| {
                AppError::new(
                    "SESSION-019",
                    format!(
                        "会话文件格式异常（{} 第 {} 行）：{error}",
                        path.display(),
                        line_index
                    ),
                )
            })?;
            let old_provider = session_meta(&value).map(|(_, provider)| provider.clone());
            if let Some(old_provider) = old_provider {
                let label = old_provider
                    .as_str()
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| "null".to_string());
                if set_session_provider(&mut value, Value::String(target.to_string())) {
                    originals.insert(label);
                    modified += 1;
                    serde_json::to_writer(&mut writer, &value).map_err(|error| {
                        AppError::new("SESSION-020", format!("无法序列化会话元数据：{error}"))
                    })?;
                    writer.write_all(ending.as_bytes()).map_err(|error| {
                        AppError::io("SESSION-021", "无法写入会话临时文件", &error)
                    })?;
                    continue;
                }
            }
            writer
                .write_all(line.as_bytes())
                .map_err(|error| AppError::io("SESSION-022", "无法复制会话 JSONL 行", &error))?;
        }
        writer
            .flush()
            .map_err(|error| AppError::io("SESSION-023", "无法刷新会话临时文件", &error))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| AppError::io("SESSION-024", "无法同步会话临时文件", &error))?;
        Ok::<(), AppError>(())
    })();

    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    drop(writer);
    if modified == 0 {
        let _ = fs::remove_file(&temporary);
        return Ok(None);
    }
    atomic::replace_file(&temporary, path)?;
    let after_hash = sha256_file(path)?;
    Ok(Some(SessionFileRecord {
        relative_path: relative_string(home, path)?,
        before_sha256: before_hash,
        after_sha256: after_hash,
        original_provider: provider_label(&originals),
        new_provider: target.to_string(),
        modified_records: modified,
        migrated_at: Utc::now().to_rfc3339(),
    }))
}

#[derive(Debug)]
struct SessionTable {
    name: String,
    primary_key: String,
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn session_tables(connection: &Connection) -> AppResult<Vec<SessionTable>> {
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .map_err(|error| AppError::new("SESSION-025", format!("无法读取数据库表：{error}")))?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| AppError::new("SESSION-026", format!("无法枚举数据库表：{error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| AppError::new("SESSION-027", format!("无法读取数据库表名：{error}")))?;
    let mut output = Vec::new();
    for name in names {
        if !SESSION_TABLES.contains(&name.as_str()) {
            continue;
        }
        let sql = format!("PRAGMA table_info({})", quote_identifier(&name));
        let mut columns = connection
            .prepare(&sql)
            .map_err(|error| AppError::new("SESSION-028", format!("无法检查表结构：{error}")))?;
        let rows = columns
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
            })
            .map_err(|error| AppError::new("SESSION-029", format!("无法读取表字段：{error}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| AppError::new("SESSION-030", format!("无法读取表字段：{error}")))?;
        if !rows.iter().any(|(column, _)| column == "model_provider") {
            continue;
        }
        let primary_key = rows
            .iter()
            .find(|(_, position)| *position > 0)
            .map(|(column, _)| column.clone())
            .unwrap_or_else(|| "rowid".to_string());
        output.push(SessionTable { name, primary_key });
    }
    Ok(output)
}

fn migrate_sqlite(home: &Path, path: &Path, target: &str) -> AppResult<Option<SessionFileRecord>> {
    let before_hash = sha256_file(path)?;
    let mut connection = Connection::open(path)
        .map_err(|error| AppError::new("SESSION-031", format!("无法打开会话数据库：{error}")))?;
    connection
        .busy_timeout(Duration::from_secs(3))
        .map_err(|error| AppError::new("SESSION-032", format!("无法设置数据库锁等待：{error}")))?;
    let tables = session_tables(&connection)?;
    let transaction = connection
        .transaction()
        .map_err(|error| AppError::new("SESSION-033", format!("无法启动数据库事务：{error}")))?;
    let mut modified = 0_usize;
    let mut originals = BTreeSet::new();
    for table in &tables {
        let table_name = quote_identifier(&table.name);
        let query = format!(
            "SELECT DISTINCT COALESCE(CAST(model_provider AS TEXT), 'null') FROM {table_name}"
        );
        let mut statement = transaction.prepare(&query).map_err(|error| {
            AppError::new("SESSION-034", format!("无法读取原 Provider：{error}"))
        })?;
        let values = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| AppError::new("SESSION-035", format!("无法枚举原 Provider：{error}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                AppError::new("SESSION-036", format!("无法读取原 Provider：{error}"))
            })?;
        originals.extend(values);
        drop(statement);
        let update = format!(
            "UPDATE {table_name} SET model_provider = ?1 \
             WHERE model_provider IS NULL OR CAST(model_provider AS TEXT) <> ?1"
        );
        modified += transaction
            .execute(&update, params![target])
            .map_err(|error| {
                AppError::new(
                    "SESSION-037",
                    format!("会话数据库 Provider 迁移失败：{error}"),
                )
            })?;
    }
    transaction
        .commit()
        .map_err(|error| AppError::new("SESSION-038", format!("会话数据库提交失败：{error}")))?;
    drop(connection);
    if modified == 0 {
        return Ok(None);
    }
    let after_hash = sha256_file(path)?;
    Ok(Some(SessionFileRecord {
        relative_path: relative_string(home, path)?,
        before_sha256: before_hash,
        after_sha256: after_hash,
        original_provider: provider_label(&originals),
        new_provider: target.to_string(),
        modified_records: modified,
        migrated_at: Utc::now().to_rfc3339(),
    }))
}

pub fn migrate_all(home: &Path, target: &str) -> AppResult<MigrationSummary> {
    let mut summary = MigrationSummary::default();
    for path in candidate_files(home)? {
        let record = if path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("state_5.sqlite"))
        {
            migrate_sqlite(home, &path, target)?
        } else {
            migrate_jsonl(home, &path, target)?
        };
        if let Some(record) = record {
            summary.total_changes += record.modified_records;
            summary.records.push(record);
        }
    }
    Ok(summary)
}

fn original_jsonl_providers(path: &Path) -> AppResult<HashMap<String, Value>> {
    let mut providers = HashMap::new();
    if !path.is_file() {
        return Ok(providers);
    }
    let input = File::open(path)
        .map_err(|error| AppError::io("SESSION-039", "无法读取原始会话备份", &error))?;
    for (line_index, line) in BufReader::new(input).lines().enumerate() {
        let line =
            line.map_err(|error| AppError::io("SESSION-040", "无法读取原始会话行", &error))?;
        let value: Value = serde_json::from_str(&line).map_err(|error| {
            AppError::new(
                "SESSION-041",
                format!("原始会话备份格式异常（第 {} 行）：{error}", line_index + 1),
            )
        })?;
        if let Some((id, provider)) = session_meta(&value) {
            providers.insert(id.to_string(), provider.clone());
        }
    }
    Ok(providers)
}

fn restore_jsonl(
    path: &Path,
    original_path: Option<&Path>,
    fallback: &str,
) -> AppResult<Option<SessionFileRecord>> {
    let originals = match original_path {
        Some(path) => original_jsonl_providers(path)?,
        None => HashMap::new(),
    };
    let before_hash = sha256_file(path)?;
    let temporary = atomic::temporary_sibling(path)?;
    let input = File::open(path)
        .map_err(|error| AppError::io("SESSION-042", "无法读取待恢复会话", &error))?;
    let output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| AppError::io("SESSION-043", "无法创建恢复临时文件", &error))?;
    let mut writer = BufWriter::new(output);
    let mut modified = 0_usize;

    let result = (|| {
        let mut reader = BufReader::new(input);
        let mut line = String::new();
        let mut line_index = 0_usize;
        loop {
            line.clear();
            let read = reader
                .read_line(&mut line)
                .map_err(|error| AppError::io("SESSION-044", "无法读取待恢复会话行", &error))?;
            if read == 0 {
                break;
            }
            line_index += 1;
            let (content, ending) = line_ending(&line);
            let mut value: Value = serde_json::from_str(content).map_err(|error| {
                AppError::new(
                    "SESSION-045",
                    format!("待恢复会话格式异常（第 {} 行）：{error}", line_index),
                )
            })?;
            let target = session_meta(&value).and_then(|(id, provider)| {
                let managed = provider
                    .as_str()
                    .is_some_and(|value| MANAGED_PROVIDER_IDS.contains(&value));
                if !managed {
                    return None;
                }
                Some(
                    originals
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| Value::String(fallback.to_string())),
                )
            });
            if let Some(target) = target {
                if set_session_provider(&mut value, target) {
                    modified += 1;
                    serde_json::to_writer(&mut writer, &value).map_err(|error| {
                        AppError::new("SESSION-046", format!("无法序列化恢复会话：{error}"))
                    })?;
                    writer
                        .write_all(ending.as_bytes())
                        .map_err(|error| AppError::io("SESSION-047", "无法写入恢复会话", &error))?;
                    continue;
                }
            }
            writer
                .write_all(line.as_bytes())
                .map_err(|error| AppError::io("SESSION-048", "无法复制恢复会话行", &error))?;
        }
        writer
            .flush()
            .map_err(|error| AppError::io("SESSION-049", "无法刷新恢复会话", &error))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| AppError::io("SESSION-050", "无法同步恢复会话", &error))?;
        Ok::<(), AppError>(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    drop(writer);
    if modified == 0 {
        let _ = fs::remove_file(&temporary);
        return Ok(None);
    }
    atomic::replace_file(&temporary, path)?;
    Ok(Some(SessionFileRecord {
        relative_path: path.to_string_lossy().into_owned(),
        before_sha256: before_hash,
        after_sha256: sha256_file(path)?,
        original_provider: "managed".to_string(),
        new_provider: fallback.to_string(),
        modified_records: modified,
        migrated_at: Utc::now().to_rfc3339(),
    }))
}

fn restore_sqlite(path: &Path, original_path: Option<&Path>, fallback: &str) -> AppResult<usize> {
    let mut connection = Connection::open(path)
        .map_err(|error| AppError::new("SESSION-051", format!("无法打开待恢复数据库：{error}")))?;
    connection
        .busy_timeout(Duration::from_secs(3))
        .map_err(|error| AppError::new("SESSION-052", format!("无法设置数据库锁等待：{error}")))?;
    let tables = session_tables(&connection)?;
    if let Some(original) = original_path.filter(|path| path.is_file()) {
        let original = original.to_string_lossy().into_owned();
        connection
            .execute("ATTACH DATABASE ?1 AS baseline", params![original])
            .map_err(|error| {
                AppError::new("SESSION-053", format!("无法附加原始数据库：{error}"))
            })?;
    }
    let has_baseline = original_path.is_some_and(Path::is_file);
    let transaction = connection
        .transaction()
        .map_err(|error| AppError::new("SESSION-054", format!("无法启动恢复事务：{error}")))?;
    let mut modified = 0_usize;
    for table in &tables {
        let table_name = quote_identifier(&table.name);
        let primary_key = quote_identifier(&table.primary_key);
        if has_baseline {
            let sql = format!(
                "UPDATE main.{table_name} AS current SET model_provider = (\
                   SELECT original.model_provider FROM baseline.{table_name} AS original \
                   WHERE CAST(original.{primary_key} AS TEXT) = CAST(current.{primary_key} AS TEXT)\
                 ) WHERE CAST(current.model_provider AS TEXT) = ?1 \
                 AND EXISTS (SELECT 1 FROM baseline.{table_name} AS original \
                   WHERE CAST(original.{primary_key} AS TEXT) = CAST(current.{primary_key} AS TEXT))"
            );
            modified += transaction
                .execute(&sql, params![GENERIC_PROVIDER_ID])
                .map_err(|error| {
                    AppError::new(
                        "SESSION-055",
                        format!("恢复数据库原 Provider 失败：{error}"),
                    )
                })?;
            let new_rows = format!(
                "UPDATE main.{table_name} AS current SET model_provider = ?1 \
                 WHERE CAST(current.model_provider AS TEXT) = ?2 \
                 AND NOT EXISTS (SELECT 1 FROM baseline.{table_name} AS original \
                   WHERE CAST(original.{primary_key} AS TEXT) = CAST(current.{primary_key} AS TEXT))"
            );
            modified += transaction
                .execute(&new_rows, params![fallback, GENERIC_PROVIDER_ID])
                .map_err(|error| {
                    AppError::new(
                        "SESSION-056",
                        format!("恢复新增会话 Provider 失败：{error}"),
                    )
                })?;
        } else {
            let sql = format!(
                "UPDATE {table_name} SET model_provider = ?1 \
                 WHERE CAST(model_provider AS TEXT) = ?2"
            );
            modified += transaction
                .execute(&sql, params![fallback, GENERIC_PROVIDER_ID])
                .map_err(|error| {
                    AppError::new("SESSION-057", format!("恢复数据库 Provider 失败：{error}"))
                })?;
        }
    }
    transaction
        .commit()
        .map_err(|error| AppError::new("SESSION-058", format!("恢复数据库提交失败：{error}")))?;
    if has_baseline {
        connection
            .execute("DETACH DATABASE baseline", [])
            .map_err(|error| {
                AppError::new("SESSION-059", format!("无法分离原始数据库：{error}"))
            })?;
    }
    Ok(modified)
}

pub fn restore_from_baseline(
    home: &Path,
    baseline_root: &Path,
    fallback: &str,
) -> AppResult<MigrationSummary> {
    let mut summary = MigrationSummary::default();
    for current in candidate_files(home)? {
        let relative = relative_string(home, &current)?;
        let original = backup_path(baseline_root, &relative);
        if current
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("state_5.sqlite"))
        {
            summary.total_changes += restore_sqlite(
                &current,
                original.is_file().then_some(original.as_path()),
                fallback,
            )?;
        } else if let Some(record) = restore_jsonl(
            &current,
            original.is_file().then_some(original.as_path()),
            fallback,
        )? {
            summary.total_changes += record.modified_records;
            summary.records.push(record);
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const META: &str = r#"{"timestamp":"2026-07-25T00:00:00Z","type":"session_meta","payload":{"id":"session-1","cwd":"C:\\project","model_provider":"openai"}}"#;
    const MESSAGE: &str = r#"{"type":"response_item","payload":{"role":"user","content":"do not change model_provider in this message"}}"#;

    #[test]
    fn jsonl_migration_changes_only_session_metadata() {
        let home = TempDir::new().unwrap();
        let directory = home.path().join("sessions");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("rollout.jsonl");
        fs::write(&path, format!("{META}\n{MESSAGE}\n")).unwrap();
        let summary = migrate_all(home.path(), GENERIC_PROVIDER_ID).unwrap();
        assert_eq!(summary.total_changes, 1);
        let output = fs::read_to_string(path).unwrap();
        assert!(output.contains(r#""model_provider":"vexlune_hub""#));
        assert!(output.contains(MESSAGE));
        assert!(output.contains(r#""id":"session-1""#));
        assert!(output.contains(r#""cwd":"C:\\project""#));
    }

    #[test]
    fn streamed_backup_records_the_complete_file_hash_and_size() {
        let home = TempDir::new().unwrap();
        let directory = home.path().join("sessions");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("large-rollout.jsonl");
        let payload = vec![b'x'; 2 * 1024 * 1024 + 137];
        fs::write(&path, &payload).unwrap();
        let backup = home.path().join("switcher-backups").join("streamed");
        fs::create_dir_all(&backup).unwrap();

        let records = backup_all(home.path(), &backup).unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].size, payload.len() as u64);
        assert_eq!(records[0].sha256, sha256_file(&path).unwrap());
        assert_eq!(
            records[0].sha256,
            sha256_file(&backup.join(&records[0].backup_relative_path)).unwrap()
        );
    }

    #[test]
    fn malformed_jsonl_is_left_unchanged() {
        let home = TempDir::new().unwrap();
        let directory = home.path().join("sessions");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("broken.jsonl");
        fs::write(&path, "{broken\n").unwrap();
        let before = sha256_file(&path).unwrap();
        assert!(migrate_all(home.path(), "qinwen").is_err());
        assert_eq!(sha256_file(&path).unwrap(), before);
    }

    #[test]
    fn baseline_restore_recovers_original_provider() {
        let home = TempDir::new().unwrap();
        let directory = home.path().join("sessions");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("rollout.jsonl");
        fs::write(&path, format!("{META}\n{MESSAGE}\n")).unwrap();
        let backup = home.path().join("switcher-backups").join("baseline");
        fs::create_dir_all(&backup).unwrap();
        backup_all(home.path(), &backup).unwrap();
        migrate_all(home.path(), GENERIC_PROVIDER_ID).unwrap();
        restore_from_baseline(home.path(), &backup, "openai").unwrap();
        let output = fs::read_to_string(path).unwrap();
        assert!(output.contains(r#""model_provider":"openai""#));
        assert!(output.contains(MESSAGE));
    }

    #[test]
    fn restore_leaves_other_provider_sessions_unchanged() {
        let home = TempDir::new().unwrap();
        let directory = home.path().join("sessions");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("rollout.jsonl");
        let other_provider = META.replace(
            r#""model_provider":"openai""#,
            r#""model_provider":"qinwen""#,
        );
        fs::write(&path, format!("{other_provider}\n{MESSAGE}\n")).unwrap();
        let before = sha256_file(&path).unwrap();

        let baseline = home.path().join("switcher-backups").join("baseline");
        fs::create_dir_all(&baseline).unwrap();
        backup_all(home.path(), &baseline).unwrap();
        let summary = restore_from_baseline(home.path(), &baseline, "openai").unwrap();

        assert_eq!(summary.total_changes, 0);
        assert_eq!(sha256_file(&path).unwrap(), before);
    }
}
