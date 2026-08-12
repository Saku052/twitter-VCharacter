use anyhow::Result;
use async_trait::async_trait;

use crate::domain::TimeSlot;

/// 予定を外部のタスク管理サービスへ書き込む。
/// Phase B では TickTick Open API が実装する。
#[async_trait]
pub trait TaskWriter {
    /// 予定を作成し、サービス側の識別子を返す
    async fn create_task(&self, req: NewTask) -> Result<CreatedTask>;

    /// 予定を削除する
    async fn delete_task(&self, project_id: &str, task_id: &str) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct NewTask {
    pub title: String,
    pub content: Option<String>,
    pub slot: TimeSlot,
    /// 未指定なら実装側の既定プロジェクトへ入れる
    pub project_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CreatedTask {
    pub task_id: String,
    /// 削除時に projectId が要るため、作成時のものを保持する
    pub project_id: String,
}
