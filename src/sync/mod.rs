pub mod curseforge;
pub mod modrinth;

/// 单个条目的同步结果
///
/// Python 版用 `ProjectDetail` / `False` / `None` 三种返回值表达，
/// 调用方分不清「不存在」和「同步失败」，失败的 id 直接丢掉
#[derive(Debug)]
pub enum Outcome<T> {
    Synced(T),
    /// 上游返回 404，重试没有意义
    NotFound,
    /// 不属于要收录的范围
    Skipped,
}

/// 一批条目的同步结果，失败的 id 会被保留以便写回队列
#[derive(Debug)]
pub struct Report<I, T> {
    pub synced: Vec<T>,
    pub not_found: Vec<I>,
    pub skipped: Vec<I>,
    pub failed: Vec<(I, String)>,
}

impl<I, T> Default for Report<I, T> {
    fn default() -> Self {
        Self {
            synced: Vec::new(),
            not_found: Vec::new(),
            skipped: Vec::new(),
            failed: Vec::new(),
        }
    }
}

impl<I, T> Report<I, T> {
    pub fn record(&mut self, id: I, result: crate::error::Result<Outcome<T>>) {
        match result {
            Ok(Outcome::Synced(value)) => self.synced.push(value),
            Ok(Outcome::NotFound) => self.not_found.push(id),
            Ok(Outcome::Skipped) => self.skipped.push(id),
            Err(error) => self.failed.push((id, error.to_string())),
        }
    }

    pub fn total(&self) -> usize {
        self.synced.len() + self.not_found.len() + self.skipped.len() + self.failed.len()
    }
}
