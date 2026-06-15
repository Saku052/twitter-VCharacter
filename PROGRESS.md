# PROGRESS.md

各コンポーネントの現在地と次にやることを記録する。詳細な設計の前提は `CLAUDE.md` を参照。

## twitter-VCharacter

- メモ取得 → AI生成 → X投稿 → `mark_used_memo` で使用済みマークの一連の流れは実装済み
- Railwayにデプロイ済み（`RAILPACK_RUST_VERSION=stable`, `DATABASE_URL`設定済み）

### 残課題
- Twitter投稿で401 Unauthorizedが出ることがある（APIキー権限の確認が必要）
- `memo_mq.memo` カラムにNOT NULL制約をつけるか検討（現在は`Option<String>` + `unwrap_or_default()`で対応）

## data-collector

YouTubeプレイリストからタイトル・説明文を取得するアダプターを実装中。

### 完了
- `ports::youtube_port::YoutubePort`（`get_youtube_video`）と `adapters::youtube::YoutubeClient` の骨組み
- `Cargo.toml` に必要な依存追加（anyhow, async-trait, dotenvy, reqwest[json,query], serde, serde_json, sqlx, tokio）
- `config::build_app()` で `YoutubeClient` と `OpenAiClient` をDI

### 残課題（命名規則）
- `youtube_port` → `YoutubePort`、`youtube_client` → `YoutubeClient`、`youtubeResponse` → `YoutubeResponse`、`youtubeItem` → `YoutubeItem` などPascalCaseへの修正が未対応（warningとして出ている）
- `postgres.rs` をtwitter-VCharacterからコピーしたが、data-collectorでの用途（何のテーブルを使うか）が未確定

### 次にやること（大まか）
1. コンパイルを通す（依存追加・命名規則・タイポ修正）
2. YouTube取得結果（title・description両方）をChatGPTに渡してメモを生成する処理を実装
3. 生成したメモをPostgresに保存する（`memo_mq`へのINSERT処理が現状存在しない。`MemoQueue`には取得系メソッドしかない）
4. アーキテクチャ整理（domain層の追加、postgres.rs/memo_queue.rsがdata-collectorに本当に必要か精査）
