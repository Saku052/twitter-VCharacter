# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

Twitter VTuberキャラクター「さく」（ゲーム制作が好きな社会人1年目エンジニア）のツイートを自動生成・投稿するRustボット。Hexagonal Architecture（Ports & Adapters）を学習しながら実践するためのプロジェクトでもある。

Cargoワークスペースではなく、独立した2つのRustプロジェクトで構成されている:

- **twitter-VCharacter/** — メモ(DB)からツイートを生成してXに投稿するメインボット。Railwayにデプロイ
- **data-collector/** — YouTubeプレイリストなどから情報を収集してメモを作るためのコンポーネント（開発中）

## よく使うコマンド

各コンポーネントのディレクトリに `cd` してから実行する。

```bash
# ビルドチェック（DB接続が必要なクエリがあるためDATABASE_URLが必要）
cargo check

# 実行
cargo run

# sqlxのコンパイル時クエリチェック用キャッシュ生成（DBオフラインビルド用）
cargo sqlx prepare
```

`sqlx::query!` / `query_as!` マクロはビルド時に実際のDBに接続してクエリを検証するため、`DATABASE_URL`（`.env`）が有効な接続先を指していないと `cargo check` / `cargo build` がエラーになる。RailwayのDBを使う場合、ローカルからは `DATABASE_PUBLIC_URL`（外部公開用エンドポイント）の値を `.env` の `DATABASE_URL` に設定する。

## アーキテクチャ（Hexagonal Architecture）

各コンポーネントは以下の構造に従う:

```
src/
├── main.rs       # エントリーポイント。build_app()を呼び、portのメソッドだけを使ってフローを記述
├── config.rs     # build_app(): .envを読み込み、各adapterを生成してportの型で返す（DI）
├── domain/       # 外部依存のない純粋なロジック・データモデル
├── ports/        # トレイト定義（インターフェース）。#[async_trait]で非同期メソッドを宣言
└── adapters/     # portsトレイトを実装する具体的なクライアント（DB, 外部API等）
```

**依存の流れ**: `main.rs` は `ports` のトレイトのみを通じて操作し、`adapters` の具体型を直接知らない。`config.rs` の `build_app()` が `Result<(impl PortA, impl PortB, ...)>` の形で具体的なadapterをportの型として返すことで、mainは実装の詳細から分離される。

### twitter-VCharacter の構成例

- `ports::ai_generator::AiGenerator` ← `adapters::openai::OpenAiClient`（OpenAI Chat Completions API）
- `ports::text_publisher::TextPublisher` ← `adapters::twitter::TwitterClient`（OAuth 1.0a署名付きでX API v2に投稿）
- `ports::memo_queue::MemoQueue` ← `adapters::postgres::PostgresClient`（`memo_mq`テーブルをキューとして使用）

### MemoQueueパターン（memo_mqテーブル）

`memo_mq` テーブルは「未使用のメモ」を保持するキュー。`used_at IS NULL` の最も古い行を1件取得し（`fetch_latest_memo`）、ツイート投稿に成功したら `mark_used_memo(id)` で `used_at = NOW()` を設定して使用済みにする。`sqlx::query_as!` でSELECT、`sqlx::query!` でUPDATEを行う（戻り値をstructにマッピングしない場合は `query_as!` ではなく `query!` を使う）。

## 環境変数（.env）

twitter-VCharacter:
- `OPENAI_API_KEY`
- `TWITTER_API_KEY`, `TWITTER_API_SECRET_KEY`, `TWITTER_ACCESS_TOKEN`, `TWITTER_ACCESS_TOKEN_SECRET`
- `DATABASE_URL`（PostgreSQL接続文字列）

data-collector:
- `YOUTUBE_API_KEY`
- `OPENAI_API_KEY`

## デプロイ（Railway）

twitter-VCharacterはRailpackでビルドされる。`RAILPACK_RUST_VERSION` をRailwayのVariablesで `stable` に設定する必要がある（依存クレートが新しいRustバージョンを要求するため）。`DATABASE_URL` はRailwayのVariablesで `${{Postgres.DATABASE_URL}}`（クォートなし）として設定する。

sqlxのマクロはビルド時にDB接続を要求するため、ビルドコンテナからDBに到達できない場合は `cargo sqlx prepare` で生成した `.sqlx/` キャッシュをコミットし、`SQLX_OFFLINE=true` をRailwayのVariablesに設定する。

## 私（ユーザー）について

- Rustは学習中。async/await、トレイト、所有権・借用などの概念を一つずつ確認しながら進めたい
- Hexagonal Architectureの理解を深めることが目的の一つ。ports/adaptersの役割分担に疑問を持ったらまず説明する
- 日本語でやりとりする

## Claude Code としての振る舞い方

1. **コードを勝手に修正しない**
   - 「直していい？」と確認してから変更する
   - ユーザーが「は？勝手に直さないで」と言ったことがある — 提案 → 承諾 → 実行の順を守る

2. **コードは基本ユーザーが書く、Claudeはレビューと解説が中心**
   - 「レビューして」と言われたら、ファイルを読んで問題点を指摘する（致命的なエラー→改善点→良い点の順）
   - 明示的に「実装して」「修正して」と頼まれたときだけ書く

3. **概念の説明は丁寧に、かつ具体的に**
   - Rustの文法・概念（所有権、トレイト、deriveなど）で詰まったら、コードの該当箇所に即して説明する
   - 「なぜそうなるのか」を、抽象論ではなくこのプロジェクトのコードを例に説明する

4. **タイポ・命名規則の指摘は都度行う**
   - PascalCase/snake_caseの規約違反、typo（`aync`, `Deserilize`など）は早めに指摘する

## PROGRESS.md の運用

進行中のタスク（data-collectorのYouTube adapter実装など）の現在地を `PROGRESS.md`（リポジトリルート）に記録する。区切りの良いタイミングで更新する：

- 完了したこと
- 今わかっているエラー・残課題
- 次にやること
