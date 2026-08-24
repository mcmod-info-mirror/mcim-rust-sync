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

#[cfg(test)]
mod tests {
    use super::{Outcome, Report};
    use crate::error::Error;

    /// 失败的 id 必须能被带回来，Python 版拿不到 id 所以失败项直接丢了
    #[test]
    fn failures_keep_their_ids() {
        let mut report: Report<i32, &str> = Report::default();
        report.record(1, Ok(Outcome::Synced("ok")));
        report.record(2, Ok(Outcome::NotFound));
        report.record(3, Ok(Outcome::Skipped));
        report.record(4, Err(Error::Config("炸了".to_string())));
        report.record(5, Err(Error::Config("又炸了".to_string())));

        assert_eq!(report.synced, vec!["ok"]);
        assert_eq!(report.not_found, vec![2]);
        assert_eq!(report.skipped, vec![3]);
        let failed: Vec<i32> = report.failed.iter().map(|(id, _)| *id).collect();
        assert_eq!(failed, vec![4, 5]);
        assert_eq!(report.total(), 5);
    }

    /// 404 与「不收录」不该被放回队列，否则会无限循环
    #[test]
    fn only_real_failures_are_worth_requeueing() {
        let mut report: Report<i32, &str> = Report::default();
        report.record(7, Ok(Outcome::NotFound));
        report.record(8, Ok(Outcome::Skipped));
        assert!(report.failed.is_empty());
    }
}
