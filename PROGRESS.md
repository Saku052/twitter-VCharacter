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
現状コンパイルは通っている（warning 8件のみ、エラーなし）。

### 完了
- `ports::youtube_port::YoutubePort`（`get_youtube_video`）と `adapters::youtube::YoutubeClient`（YouTube Data API v3 `playlistItems` 呼び出し）
- `ports::ai_generator::AiGenerator` と `adapters::openai::OpenAiClient`（骨組み、未配線）
- `ports::memo_queue::MemoQueue` と `adapters::postgres::PostgresClient`（twitter-VCharacterからコピー、現状は読み取り系のみ）
- `Cargo.toml` に必要な依存追加（anyhow, async-trait, dotenvy, reqwest[json,query], serde, serde_json, sqlx, tokio）
- `config::build_app()` で `YoutubeClient` と `OpenAiClient` をDI
- `main.rs` で YouTube から title / description を取得して println するところまで動く

### 残課題（コード品質）
- `ports::ai_generator::AiGenerator::generate` と `adapters::openai::OpenAiClient::generate` で引数順が食い違っている
  - port: `(memo, system, model)` / adapter: `(memo, model, system)`
  - 今は呼ばれていないが、繋いだ瞬間にバグる
- `adapters/youtube.rs` の `body.items[0]` が空ベクタでパニックする（`.first().ok_or_else(...)` に直す）
- `adapters/youtube.rs` の query 引数 `&self.api_key.as_str()` が `&&str` になっている（`self.api_key.as_str()` で十分）
- `YoutubePort` の戻り値が `(String, String)` タプルで title/description の区別が呼び出し側で読めない → domain 型 `YoutubeVideo { title, description }` に昇格する案
- `postgres.rs` は読み取り系のみ。data-collector は書き込みが本業なので INSERT 用のメソッド／ポートが必要
- `main.rs:14` の「主力」は「出力」のtypo

### 設計方針（決定済み）

memo_mq テーブルの読み書きはコンポーネントごとに非対称:

- twitter-VCharacter は読み取り専用（consumer） → `MemoQueue`（既存）
- data-collector は書き込み専用（producer） → `MemoWriter`（新規）

Hexagonal の流儀に従い、「アプリ側が必要としているもの」でポートを分割する（ISP）。`PostgresClient` は各プロジェクトに置き、必要な trait だけを impl する。

### 次にやること

1. `data-collector/src/ports/memo_writer.rs` を新規作成
   - `trait MemoWriter { async fn insert_memo(&self, memo: &str) -> Result<()> }`
2. `data-collector/src/ports/mod.rs` に `pub mod memo_writer;` を追加
3. `data-collector/src/adapters/postgres.rs` を書き換え
   - `impl MemoQueue for PostgresClient` を削除
   - `impl MemoWriter for PostgresClient` を追加（`INSERT INTO memo_mq (memo) VALUES ($1)`）
   - 不要になった `MemoRow` と `ports/memo_queue.rs` も削除
4. `config::build_app()` の戻り値を `(impl YoutubePort, impl AiGenerator, impl MemoWriter)` に拡張、`PostgresClient::new(&env::var("DATABASE_URL")?)` を呼ぶ
5. `main.rs` で 3つのポートを使って `YouTube取得 → OpenAI生成 → Postgres保存` のフローを実装

### その他の残課題（コード品質、優先度低）
- `adapters/youtube.rs` の `body.items[0]` が空ベクタでパニックする（`.first().ok_or_else(...)` に直す）
- `adapters/youtube.rs` の query 引数 `&self.api_key.as_str()` が `&&str` になっている（`self.api_key.as_str()` で十分）
- `YoutubePort` の戻り値が `(String, String)` タプルで title/description の区別が読めない → domain 型 `YoutubeVideo { title, description }` に昇格する案
- `main.rs:14` の「主力」は「出力」のtypo
