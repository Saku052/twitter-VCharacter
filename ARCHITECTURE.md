# アーキテクチャドキュメント

対象: `twitter-VCharacter` / `data-collector`
作成: PdMインスタンス（要件定義フェーズの成果物として）
レビュー対象: チームリーダー
最終更新: 2026-07-18

---

## 1. システム概要

Twitter VTuberキャラクター「さく」（ゲーム制作が好きな社会人1年目エンジニア）のツイートを自動生成・投稿するシステム。producer/consumerの2コンポーネントで構成され、間をPostgreSQL上の1テーブル（`memo_mq`）で疎結合に繋ぐ。

```
YouTube / OpenAI          OpenAI / X API
     │                          │
     ▼                          ▼
┌─────────────┐          ┌──────────────────┐
│data-collector│  memo_mq │twitter-VCharacter│
│  (producer)  │ ───────▶ │    (consumer)     │
└─────────────┘  PostgreSQL└──────────────────┘
```

- **twitter-VCharacter**: `memo_mq` から未使用メモを取り出し、ツイート文に加工してXに投稿する。実装済み・Railwayにデプロイ済み
- **data-collector**: YouTubeプレイリストから動画情報を取得し、AIで「さく」目線のメモに変換して `memo_mq` に書き込む。開発中

2つは別々のRustプロジェクト（Cargoワークスペースではない）であり、互いのコードに依存しない。**共有DBのテーブルのみが結合点**。

---

## 2. 設計原則

### 2.1 Hexagonal Architecture（Ports & Adapters）

両プロジェクトとも以下の層構造を採用する。

```
src/
├── main.rs     エントリーポイント。portのメソッドのみを呼ぶ
├── config.rs   build_app(): .envを読み込み、adapterを生成してportの型で返す（DI）
├── domain/     外部依存のない純粋なデータモデル
├── ports/      trait定義（インターフェース）
└── adapters/   portsを実装する具体的なクライアント（DB, 外部API）
```

**依存の方向**: `main.rs` → `ports`（trait）のみに依存し、`adapters`の具体型を知らない。`config.rs::build_app()` が `Result<(impl PortA, impl PortB, ...)>` の形で組み立てる。

**採用理由**: このプロジェクトはHexagonal Architectureの学習を目的の一つとしている（[CLAUDE.md](CLAUDE.md)に明記）。実運用上のメリットとしては、外部API（YouTube/OpenAI/X/Postgres）をモックに差し替えたテストが将来書きやすい点、adapter実装を丸ごと入れ替えられる点がある。

**留意点（トレードオフ）**: 現状の規模（各プロジェクト数個のadapter）に対しては層の分離がややオーバースペック。ボイラープレートは増えるが、学習目的と将来の拡張性（例: producerの追加）を優先している。

### 2.2 プロジェクト分割の理由

Cargoワークスペースにせず、独立した2プロジェクトにしている。

- producer（data-collector）とconsumer（twitter-VCharacter）は実行タイミング・デプロイサイクルが異なる（後述の運用フロー参照）
- 依存クレートのバージョンを独立して更新できる
- 結合点をDBテーブルのみに絞ることで、互いのコード変更が相手に波及しない

---

## 3. データフロー（詳細）

### 3.1 producer側: data-collector

| # | コンポーネント | メソッド | 入力 → 出力 |
|---|---|---|---|
| 1 | `YoutubeClient` | `fetch_recent_videos()` | なし → `Vec<VideoInfo>`（直近10件、`maxResults=10`） |
| 2 | ループ内: `PostgresClient` | `is_processed(video_id)` | `video_id: &str` → `bool` |
| 3 | ループ内: `OpenAiClient` | `generate(title, description)` | `(&str, &str)` → `memo: String` |
| 4 | ループ内: `PostgresClient` | `insert_memo(memo, video_id)` | `(&str, &str)` → `()`（`processed_videos`+`memo_mq`に同一トランザクションで書き込み） |

10件は独立処理。`is_processed`が`true`ならAI生成をスキップ（無駄なOpenAI課金を避ける）。AI生成後の書き込みが失敗した場合はロールバックされ、その動画は`processed_videos`に記録されないため翌日のバッチで自動的に再挑戦される。

### 3.2 consumer側: twitter-VCharacter

| # | コンポーネント | メソッド | 入力 → 出力 |
|---|---|---|---|
| A | `PostgresClient` | `fetch_latest_memo()` | なし → `MemoRow { id, memo }`（`used_at IS NULL`の最古1件） |
| B | `OpenAiClient` | `generate(memo)` | `memo: &str` → `tweet_text: String` |
| C | `TwitterClient` | `post_tweet(tweet_text)` | `&str` → 投稿成功 |
| D | `PostgresClient` | `mark_used_memo(id)` | `id` → `used_at = NOW()` に更新 |

### 3.3 共有DBテーブル

| テーブル | 役割 | 読み書き |
|---|---|---|
| `memo_mq` | 未使用メモのキュー。`used_at IS NULL`が未消費 | data-collectorがINSERT（producer専用）、twitter-VCharacterがSELECT+UPDATE（consumer専用） |
| `processed_videos`（新設予定） | YouTube動画の処理済み管理。`video_id TEXT PRIMARY KEY` | data-collectorのみが読み書き。twitter-VCharacterは参照しない |

**非対称設計の理由**: `memo_mq`はproducer/consumerで役割が完全に分かれるため、trait自体を分ける（`MemoWriter` / `MemoQueue`）。ISP（Interface Segregation Principle）に従い、「アプリ側が必要とするメソッドだけ」でポートを定義する。

**`processed_videos`を`memo_mq`と別テーブルにする理由**: 重複排除はYouTube固有の関心事であり、将来data-collector以外のproducer（他SNS・RSS等）が増えた場合に`memo_mq`側へYouTube固有の列（`video_id`等）を持ち込みたくないため。producerごとの重複排除ロジックをテーブルレベルで分離しておく。

---

## 4. 運用フロー

| 項目 | data-collector | twitter-VCharacter |
|---|---|---|
| 実行環境 | Railway, cron | Railway, cron |
| 頻度 | 1日1回 | 1日1回 |
| 1回の処理量 | 直近10件取得 → 未処理分のみ処理 | 未使用メモ1件を消費 |

**需給バランス**: YouTubeプレイリストへの新規動画追加は週数件程度（低頻度）と想定。消費側は週7件相当（1日1回×7日）のため、**供給 < 消費**の構造になる。これは想定内で、data-collectorは`memo_mq`の唯一の供給源ではなく、手動投入と併存する補助的な供給源という位置付け。単独で在庫を維持しきる設計にはしていない。

**品質保証方針**: メモ生成〜`memo_mq`投入までは完全自動、人間レビューは挟まない。品質はAIへのシステムプロンプト（50文字以内・ハッシュタグ禁止・事実要約ではなく感想化）でのみ担保する。レビュー用の中間ステータスやワークフローは持たない。

---

## 5. 障害時の挙動

| 障害 | 挙動 |
|---|---|
| data-collector: YouTube API失敗 | バッチ全体が失敗（現状は動画取得が全処理の起点のため） |
| data-collector: 特定動画のAI生成失敗 | その1件のみスキップ。`processed_videos`に記録されないため翌日自動再挑戦 |
| data-collector: 特定動画のDB書き込み失敗 | トランザクションでロールバック。`processed_videos`と`memo_mq`のどちらか一方だけ書き込まれる状態は発生しない |
| twitter-VCharacter: X API 401 | 既知の残課題。APIキー権限の確認が必要（未解決、[PROGRESS.md](PROGRESS.md)参照） |

**未設計**: バッチ失敗時のアラート通知・監視は現状ログ出力のみ。障害の検知はRailwayのログを人間が確認する運用に依存している。

---

## 6. 技術スタック（要点のみ）

| 分類 | 選択 | 備考 |
|---|---|---|
| 言語/ランタイム | Rust + tokio | Hexagonal Architecture学習が目的の一つ |
| DB | PostgreSQL（Railway） | `sqlx`のコンパイル時クエリ検証を活用。マイグレーションツールは未導入で、スキーマ変更は手動SQL実行 |
| 外部API | YouTube Data API v3 / OpenAI Chat Completions / X API v2 | いずれも`reqwest`でREST+JSON |
| X API認証 | OAuth 1.0aを自前実装（hmac/sha2/base64） | 既知の課題として401エラーが散発（要調査） |

---

## 7. 未決事項・レビューで特に見てほしい点

1. **`processed_videos`が新設テーブルとして妥当か** — `memo_mq`に列追加する案も検討したが、将来の他producer拡張性を優先して別テーブルにした。この判断への異論があれば要相談
2. **供給 < 消費の需給ギャップを許容している設計** — data-collectorだけでは`memo_mq`を満たしきれない。手動投入运用が前提だが、将来的に自動化を強化する必要が出た場合はproducer側の複数ソース化（YouTube以外）を検討する
3. **X API OAuth1.0aの自前実装** — 既存クレート（例: `oauth1`系）への切り替えを検討する価値があるかもしれない。現状は動いているが401エラーの原因調査が済んでいない
4. **障害通知の仕組みがない** — 1日1回のバッチが静かに失敗し続けても気づく手段が今はログ確認のみ。運用が本格化する場合はアラート設計が必要

---

## 関連ドキュメント

- [CLAUDE.md](CLAUDE.md) — プロジェクト運用ルール、Claude Codeとしての振る舞い方
- [PROGRESS.md](PROGRESS.md) — 現在の実装進捗、次のアクション
- [data-collector/HANDOFF.md](data-collector/HANDOFF.md) — Dev向け実装指令書（今回のデイリーバッチ機能）
