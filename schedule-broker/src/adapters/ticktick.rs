use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::ports::task_writer::{CreatedTask, NewTask, TaskWriter};

const DEFAULT_BASE_URL: &str = "https://api.ticktick.com/open/v1";

/// TickTick Open API で予定を作成する。
///
/// 注意: access_token に有効期限の情報が返らない仕様のため、
/// refresh によるローテーションは行わず、取得済みトークンをそのまま使う。
/// 期限切れは 401 として表面化する。
pub struct TickTickClient {
    access_token: String,
    /// 未指定時の登録先。空なら受信トレイ（Inbox）へ入る
    default_project_id: Option<String>,
    base_url: String,
}

impl TickTickClient {
    pub fn new(access_token: String, default_project_id: Option<String>) -> Self {
        Self {
            access_token,
            default_project_id: default_project_id.filter(|s| !s.is_empty()),
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    #[cfg(test)]
    fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    /// TickTick が要求する `yyyy-MM-dd'T'HH:mm:ssZ`（コロン無しオフセット）に整形する
    fn format_datetime(t: DateTime<Utc>) -> String {
        t.format("%Y-%m-%dT%H:%M:%S%z").to_string()
    }

}

#[async_trait]
impl TaskWriter for TickTickClient {
    async fn create_task(&self, req: NewTask) -> Result<CreatedTask> {
        let client = reqwest::Client::new();

        let mut body = serde_json::json!({
            "title": req.title,
            "startDate": Self::format_datetime(req.slot.start),
            "dueDate": Self::format_datetime(req.slot.end),
            "timeZone": "Asia/Tokyo",
            "isAllDay": false,
        });
        if let Some(content) = &req.content {
            body["content"] = serde_json::Value::String(content.clone());
        }

        // projectId を省略すると受信トレイ（Inbox）に入る。
        // Inbox は /open/v1/project の一覧に現れず ID を引く手段が無いため、
        // 明示指定が無いときは送らないことで Inbox を既定にする
        let project_id = req
            .project_id
            .filter(|s| !s.is_empty())
            .or_else(|| self.default_project_id.clone());
        if let Some(id) = &project_id {
            body["projectId"] = serde_json::Value::String(id.clone());
        }

        let response = client
            .post(format!("{}/task", self.base_url))
            .bearer_auth(&self.access_token)
            .json(&body)
            .send()
            .await
            .context("TickTickへのタスク作成に失敗しました")?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            bail!("タスク作成に失敗しました (status={}): {}", status, text);
        }

        let created: CreatedTaskResponse = response
            .json()
            .await
            .context("タスク作成レスポンスの解釈に失敗しました")?;

        // 取り消しには projectId が要る。Inbox の場合レスポンスでしか知り得ないため、
        // サーバが返した値を必ず使う
        let resolved = created.project_id.or(project_id).context(
            "TickTickがprojectIdを返しませんでした。取り消しができなくなるため中断します",
        )?;

        Ok(CreatedTask {
            task_id: created.id,
            project_id: resolved,
        })
    }

    /// 予約を取り消す。
    ///
    /// TickTick Open API の DELETE は 200 を返すが実際にはタスクが消えない
    /// （2026-08 時点で実機確認済み。繰り返しても消えない）。
    /// そのため complete で完了扱いにしてから DELETE も試みる。
    /// 完了済みタスクはカレンダー上で時間を占有しないため、取り消しとしては十分。
    async fn delete_task(&self, project_id: &str, task_id: &str) -> Result<()> {
        let client = reqwest::Client::new();

        let completed = client
            .post(format!(
                "{}/project/{}/task/{}/complete",
                self.base_url, project_id, task_id
            ))
            .bearer_auth(&self.access_token)
            .send()
            .await
            .context("TickTickのタスク完了に失敗しました")?;

        let status = completed.status();
        // 既に存在しないなら取り消し済みとみなす
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        if !status.is_success() {
            let text = completed.text().await.unwrap_or_default();
            bail!("タスクの取り消しに失敗しました (status={}): {}", status, text);
        }

        // DELETE が機能する環境では実際に消えるよう、併せて試みる。
        // 失敗しても completed 済みなので取り消しとしては成立している
        let deleted = client
            .delete(format!(
                "{}/project/{}/task/{}",
                self.base_url, project_id, task_id
            ))
            .bearer_auth(&self.access_token)
            .send()
            .await;

        if let Err(e) = deleted {
            tracing::warn!(
                task_id = %task_id,
                "完了にはしたが削除に失敗しました（実害なし）: {:?}",
                e
            );
        }

        Ok(())
    }
}

#[derive(Deserialize)]
struct CreatedTaskResponse {
    id: String,
    #[serde(rename = "projectId")]
    project_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TimeSlot;
    use chrono::TimeZone;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn slot() -> TimeSlot {
        TimeSlot::new(
            Utc.with_ymd_and_hms(2026, 8, 26, 10, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 8, 26, 11, 0, 0).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn formats_datetime_without_colon_in_offset() {
        // TickTickは +0000 形式を要求する（+00:00 ではない）
        let t = Utc.with_ymd_and_hms(2019, 11, 13, 3, 0, 0).unwrap();
        assert_eq!(TickTickClient::format_datetime(t), "2019-11-13T03:00:00+0000");
    }

    #[tokio::test]
    async fn creates_task_with_explicit_project() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/task"))
            .and(header("authorization", "Bearer test-token"))
            .and(body_partial_json(serde_json::json!({
                "title": "打ち合わせ",
                "projectId": "proj-1",
                "isAllDay": false,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "task-123",
                "projectId": "proj-1"
            })))
            .mount(&server)
            .await;

        let client = TickTickClient::new("test-token".into(), Some("proj-1".into()))
            .with_base_url(server.uri());

        let created = client
            .create_task(NewTask {
                title: "打ち合わせ".into(),
                content: None,
                slot: slot(),
                project_id: None,
            })
            .await
            .unwrap();

        assert_eq!(created.task_id, "task-123");
        assert_eq!(created.project_id, "proj-1");
    }

    /// projectId 未指定なら送信しない（TickTick側で受信トレイに入る）。
    /// 一覧APIに Inbox が出ないため、これが Inbox を既定にする唯一の方法
    #[tokio::test]
    async fn omits_project_id_to_use_inbox() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/task"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "task-9", "projectId": "inbox125948301"
            })))
            .mount(&server)
            .await;

        let client = TickTickClient::new("t".into(), None).with_base_url(server.uri());
        let created = client
            .create_task(NewTask {
                title: "x".into(),
                content: None,
                slot: slot(),
                project_id: None,
            })
            .await
            .unwrap();

        // 取り消しに使えるよう、サーバが返したInboxのIDを保持する
        assert_eq!(created.project_id, "inbox125948301");
    }

    /// projectId を返さないサーバには寄りかからない（取り消し不能になるため）
    #[tokio::test]
    async fn fails_when_project_id_is_unknown() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/task"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "task-9"
            })))
            .mount(&server)
            .await;

        let client = TickTickClient::new("t".into(), None).with_base_url(server.uri());
        let err = client
            .create_task(NewTask {
                title: "x".into(), content: None, slot: slot(), project_id: None,
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("projectId"), "実際: {}", err);
    }

    #[tokio::test]
    async fn surfaces_api_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/task"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let client =
            TickTickClient::new("bad".into(), Some("p".into())).with_base_url(server.uri());
        let err = client
            .create_task(NewTask {
                title: "x".into(),
                content: None,
                slot: slot(),
                project_id: None,
            })
            .await
            .unwrap_err();

        assert!(err.to_string().contains("401"), "実際: {}", err);
    }

    #[tokio::test]
    async fn cancel_treats_missing_task_as_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/project/p/task/t/complete"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = TickTickClient::new("t".into(), None).with_base_url(server.uri());
        assert!(client.delete_task("p", "t").await.is_ok(), "404は冪等に成功扱い");
    }

    /// TickTickのDELETEは200を返しても実際には消えないため、
    /// completeで完了にする経路が必ず通ることを保証する
    #[tokio::test]
    async fn cancel_marks_task_complete() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/project/p1/task/t1/complete"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let client = TickTickClient::new("tok".into(), None).with_base_url(server.uri());
        client.delete_task("p1", "t1").await.unwrap();
        // expect(1) により complete が呼ばれたことがdrop時に検証される
    }

    #[tokio::test]
    async fn cancel_surfaces_complete_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
            .mount(&server)
            .await;

        let client = TickTickClient::new("tok".into(), None).with_base_url(server.uri());
        let err = client.delete_task("p", "t").await.unwrap_err();
        assert!(err.to_string().contains("403"), "実際: {}", err);
    }
}
