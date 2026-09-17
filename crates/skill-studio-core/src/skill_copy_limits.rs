#[derive(Debug, Clone, Copy)]
pub struct BackupCopyLimits {
    pub max_bytes: u64,
    pub max_entries: u64,
    pub max_depth: usize,
}
