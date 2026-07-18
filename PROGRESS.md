# PROGRESS.md

各コンポーネントの現在地と次にやることを記録する。詳細な設計の前提は `CLAUDE.md` を参照。

## twitter-VCharacter

- メモ取得 → AI生成 → X投稿 → `mark_used_memo` で使用済みマークの一連の流れは実装済み
- Railwayにデプロイ済み（`RAILPACK_RUST_VERSION=stable`, `DATABASE_URL`設定済み）

### 残課題
- Twitter投稿で401 Unauthorizedが出ることがある（APIキー権限の確認が必要）
- `memo_mq.memo` カラムにNOT NULL制約をつけるか検討（現在は`Option<String>` + `unwrap_or_default()`で対応）

## data-collector

**現フェーズ: 開発速度優先。方針が明確な変更はClaudeが直接実装する。**

YouTubeプレイリストから直近10件の動画を取得し、未処理分だけAIでメモ化してmemo_mqに書き込むproducer。デイリーバッチ方式の実装が完了。設計の背景・理由は[ARCHITECTURE.md](ARCHITECTURE.md)を参照。

### 完了
- `ports::youtube_port::YoutubePort::fetch_recent_videos()` と `adapters::youtube::YoutubeClient`（`maxResults=10`、`Vec<VideoInfo>`を返す、空リスト対応済み）
- `domain::VideoInfo { video_id, title, description }` 新設
- `ports::memo_writer::MemoWriter`（`insert_memo(memo, video_id)`, `is_processed(video_id)`）と `adapters::postgres::PostgresClient`（`processed_videos`+`memo_mq`への同一トランザクション書き込み実装済み）
- `main.rs`: 10件ループ処理（`is_processed`判定→AI生成→`insert_memo`）、1件失敗時はログ出力してcontinue、バッチ終了時に成功件数をログ出力
- `config::build_app()` で `YoutubeClient` / `OpenAiClient` / `PostgresClient` をDI

### 次にやること（優先順）

1. `processed_videos` テーブルをRailway DBに作成（未実行）
   ```sql
   CREATE TABLE processed_videos (
       video_id TEXT PRIMARY KEY,
       processed_at TIMESTAMPTZ NOT NULL DEFAULT now()
   );
   ```
2. `cargo sqlx prepare` を実行し `.sqlx/` キャッシュを更新・コミット（Railwayビルドは`SQLX_OFFLINE=true`前提のため必須）
3. ローカルで `cargo run` を実行し、DB上で `memo_mq` / `processed_videos` に増分があることを確認 → 2回目実行で全件スキップされることを確認
4. Railway側にcronスケジュールを登録（1日1回）
5. `data-collector/HANDOFF.md` は実装完了に伴い削除済み

### 残課題（コード品質、優先度低）
- `main.rs:25` の「主力」は「出力」のtypo（コメント）
- 障害通知・アラートは未整備（ログ出力のみ。[ARCHITECTURE.md §7](ARCHITECTURE.md#7-未決事項レビューで特に見てほしい点)参照）
