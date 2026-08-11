use crate::errors::{AppError, AppResult};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:X}", hasher.finalize())
}

pub fn sha256_file(path: &Path) -> AppResult<String> {
    let mut file =
        File::open(path).map_err(|error| AppError::io("HASH-001", "无法读取待校验文件", &error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| AppError::io("HASH-002", "计算文件哈希失败", &error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:X}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_known_value() {
        assert_eq!(
            sha256_bytes(b"codex"),
            "57DE4CF40144BDF7D00010F2F5557A7D642C2B9705309BFADE167DD313E2CA93"
        );
    }
}
