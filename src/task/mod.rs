pub mod curseforge;
pub mod modrinth;

use chrono::{DateTime, Utc};

use crate::db::Queues;
use crate::error::Result;

/// 按秒比较时间戳
///
/// 库里存的是 BSON DateTime，只有毫秒精度，而 Modrinth 上游给到微秒，
/// 直接按完整精度比较会让每个项目都被判成有更新
pub fn same_second(left: Option<DateTime<Utc>>, right: Option<DateTime<Utc>>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.timestamp() == right.timestamp(),
        (None, None) => true,
        _ => false,
    }
}

/// 一次任务运行的结果统计
#[derive(Debug, Default)]
pub struct TaskSummary {
    pub total: usize,
    pub synced: usize,
    pub not_found: usize,
    pub skipped: usize,
    pub failed: usize,
    pub requeued: usize,
}

impl TaskSummary {
    pub fn log(&self, task: &str) {
        tracing::info!(
            task,
            total = self.total,
            synced = self.synced,
            not_found = self.not_found,
            skipped = self.skipped,
            failed = self.failed,
            requeued = self.requeued,
            "任务结束"
        );
    }

    /// 有失败就以非零码退出，便于外部调度识别
    pub fn is_clean(&self) -> bool {
        self.failed == 0
    }
}

/// 队列取出后若处理不成功，必须放回去
///
/// Python 版清空队列后不回填，失败的 id 会永久丢失
pub async fn requeue(queues: &Queues, key: &str, members: &[String]) -> Result<usize> {
    if members.is_empty() {
        return Ok(0);
    }
    queues.push(key, members).await?;
    tracing::info!(key, count = members.len(), "未处理成功的成员已放回队列");
    Ok(members.len())
}

#[cfg(test)]
mod tests {
    use super::same_second;
    use chrono::{DateTime, Utc};

    fn at(value: &str) -> Option<DateTime<Utc>> {
        Some(value.parse().expect("时间解析失败"))
    }

    #[test]
    fn sub_second_difference_is_not_a_change() {
        // 库里是毫秒，上游给到微秒，同一时刻不应被判成有更新
        assert!(same_second(
            at("2026-06-09T23:48:35.117Z"),
            at("2026-06-09T23:48:35.117961Z")
        ));
    }

    #[test]
    fn whole_second_difference_is_a_change() {
        assert!(!same_second(
            at("2026-06-09T23:48:35.999Z"),
            at("2026-06-09T23:48:36.000Z")
        ));
    }

    #[test]
    fn missing_on_one_side_is_a_change() {
        assert!(same_second(None, None));
        assert!(!same_second(at("2026-06-09T23:48:35Z"), None));
        assert!(!same_second(None, at("2026-06-09T23:48:35Z")));
    }
}
