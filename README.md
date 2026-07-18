# twitter-VCharacter

Twitter VTuberキャラクター「さく」（ゲーム制作が好きな社会人1年目エンジニア）のツイートを自動生成・投稿するRustボット。

## 構成

Cargoワークスペースではなく、独立した2つのRustプロジェクトで構成されている。

- **[twitter-VCharacter/](twitter-VCharacter/)** — メモ(DB)からツイートを生成してXに投稿するメインボット（consumer）。Railwayにデプロイ済み
- **[data-collector/](data-collector/)** — YouTubeプレイリストから情報を収集してメモを作るコンポーネント（producer）。開発中

2つは互いのコードに依存せず、PostgreSQL上の `memo_mq` テーブルのみで疎結合に繋がる。詳細な設計・データフローは[ARCHITECTURE.md](ARCHITECTURE.md)を参照。

## セットアップ

各コンポーネントのディレクトリに `cd` してから実行する。

```bash
# .env を作成し、必要な環境変数を設定する（一覧は CLAUDE.md の「環境変数」節を参照）

# ビルドチェック（DB接続が必要なクエリがあるため DATABASE_URL が必要）
cargo check

# 実行
cargo run
```

## ドキュメント

- [ARCHITECTURE.md](ARCHITECTURE.md) — システム全体の設計、データフロー、運用フロー、未決事項
- [CLAUDE.md](CLAUDE.md) — 開発ルール、よく使うコマンド、環境変数、デプロイ手順
- [PROGRESS.md](PROGRESS.md) — 各コンポーネントの現在地・次にやること
