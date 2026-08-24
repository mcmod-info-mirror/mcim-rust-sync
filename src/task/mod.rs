pub mod curseforge;
pub mod modrinth;

use crate::db::Queues;
use crate::error::Result;

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
