# schedule-broker

他のAIエージェントから呼ばれる、空き時間判定API。
「08/26 11:30 空いてる？」「直近1週間で22時前に空いてる時間ある？」に答える。

**Phase A（空き判定）/ Phase B（予定登録）とも実装済み。**

設計の背景は [../scratch/SECRETARY_REQUIREMENTS.md](../scratch/SECRETARY_REQUIREMENTS.md) 参照。

## なぜGoogle Calendarを読むのか

TickTick Open APIには「タスク一覧取得」が存在しない（GETはID既知の1件のみ）。
空き判定は「予定が無いこと」の確認なので、一覧が取れないと成立しない。
TickTickとGoogle Calendarは同期済みのため、**読み取りはGoogle Calendar、書き込み（Phase B）はTickTick**とする。

## セットアップ

### 1. Google OAuth クライアントを作る

1. [Google Cloud Console](https://console.cloud.google.com/) でプロジェクトを作成
2. 「APIとサービス」→「ライブラリ」で **Google Calendar API** を有効化
3. 「認証情報」→「OAuth クライアント ID を作成」→ アプリの種類は **デスクトップアプリ**
4. クライアントIDとシークレットを控える

「対象」→ テストユーザーに自分のアカウントを追加しておくこと。
これが無いと承認時に弾かれる。

### 2. .env を用意する

`.env.example` をコピーし、ダウンロードしたJSONから `client_id` / `client_secret` を書き写す。

```bash
cp .env.example .env
```

### 3. refresh_token を取得する

スクリプトが承認からトークン書き込みまで自動で行う。
トークンは端末外に出ない。

```bash
python3 scripts/get_refresh_token.py
```

ブラウザで承認する。「このアプリは確認されていません」は自作アプリなので正常
（詳細 → 安全ではないページに移動）。

スコープは `calendar.readonly`（読み取り専用）。Phase A は読むだけなので書き込み権限は要求しない。

`refresh_token が返りませんでした` と出る場合は既に承認済み。
https://myaccount.google.com/permissions で連携を解除してから再実行する。

### 4. 生活パターンを設定する

`availability.toml` は 2026-06〜08 の実カレンダー（27件）から導出済み。

- 勤務: 9:00-17:30（14件中10件。8:00-16:30 が3件）
- 平日の受付: 18:00〜（勤務終了＋移動15分）
- バッファ: 前後15分（溝の口〜二子玉川は大井町線で数分のため）

生活が変わったらこのファイルだけ直せばよい。

## 実行

```bash
cargo run
```

```bash
cargo test
```

## API

認証は全エンドポイント（`/health` を除く）で `X-API-Key` ヘッダが必要。

### `POST /v1/availability/check`

```bash
curl -X POST http://localhost:8080/v1/availability/check \
  -H 'content-type: application/json' \
  -H 'x-api-key: YOUR_KEY' \
  -d '{"start":"2026-08-26T11:30:00+09:00","duration_minutes":60}'
```

```jsonc
{
  "available": false,
  "reason": "blackout",          // busy_calendar | busy_pending | blackout | outside_window | too_short
  "detail": "勤務",
  "alternatives": [ { "start": "...", "end": "..." } ]   // 近傍の代替候補を最大3件
}
```

### `POST /v1/availability/search`

```bash
curl -X POST http://localhost:8080/v1/availability/search \
  -H 'content-type: application/json' \
  -H 'x-api-key: YOUR_KEY' \
  -d '{
    "range_start": "2026-08-12T00:00:00+09:00",
    "range_end":   "2026-08-19T00:00:00+09:00",
    "duration_minutes": 60,
    "latest_end_time": "22:00",
    "limit": 5
  }'
```

`latest_end_time` / `earliest_start_time` は "HH:MM" のローカル時刻。
「22時前に終わるもの」のような条件をここで表現する。

探索範囲は最大60日、`limit` は最大50。

### 呼び出し元の責務

**自然言語の解釈は呼び出し元が行う。** 本APIはLLMを持たない。
「直近1週間で22時前」のような表現は、呼び出し元のエージェントが
`range_start` / `range_end` / `latest_end_time` に落として渡すこと。

これにより本APIの限界費用はほぼゼロになる（呼ばれる回数が増えても課金が伸びない）。

## 判定ロジック

「予定が無い = 空いている」ではない。以下の順に評価する。

| 順 | 判定 | 理由コード |
|---|---|---|
| 1 | 要求が最小スロット長未満 | `too_short` |
| 2 | blackout（勤務・睡眠など）に重なる | `blackout` |
| 3 | window（受付可能帯）に収まらない | `outside_window` |
| 4 | カレンダーに予定がある（前後バッファ込み） | `busy_calendar` |
| 5 | 登録済み・カレンダー未反映の予約と重なる | `busy_pending` |

`busy_pending` は TickTick→Google の同期が完了する前の予約を指す。

1〜3は生活パターンだけで決まるため、**カレンダーを引かずに応答する**。
上流が落ちていても「勤務中です」は返せる。

## 構成（Hexagonal Architecture）

```
src/
├── main.rs                    # 起動のみ
├── config.rs                  # build_app(): adapterを生成しport型で返す
├── http.rs                    # axumのルーティングとDTO
├── domain/
│   ├── slot.rs                # TimeSlot（半開区間）、Availability
│   ├── pattern.rs             # availability.toml のパースと検証
│   └── availability.rs        # 空き判定エンジン（外部依存なし）
├── ports/
│   ├── calendar_reader.rs     # trait CalendarReader
│   ├── task_writer.rs         # trait TaskWriter
│   └── reservation_store.rs   # trait ReservationStore
└── adapters/
    ├── google_calendar.rs     # FreeBusy API
    ├── ticktick.rs            # Open API
    └── postgres.rs            # reservations テーブル
```

判定ロジックを外部APIから切り離してあるため、カレンダーの供給元を差し替えても
`domain/availability.rs` には手を入れずに済む。

## TickTick 連携（Phase B）

### セットアップ

1. [TickTick開発者センター](https://developer.ticktick.com/manage) で New App
2. **OAuth redirect URL** に `http://localhost:8766` を登録（これが無いと認可が通らない）
3. Client ID / Secret を `.env` に設定
4. トークン取得:

```bash
python3 scripts/get_ticktick_token.py
```

`DATABASE_URL` と `TICKTICK_ACCESS_TOKEN` の両方が揃ったときだけ書き込みが有効になる。
片方だけだと「TickTickに入ったのに記録が無い」不整合が起きるため。
無効時、予約系は 503 を返すが**空き判定は通常どおり動く**。

### `POST /v1/reservations`

```bash
curl -X POST http://localhost:8080/v1/reservations \
  -H 'content-type: application/json' -H 'x-api-key: YOUR_KEY' \
  -d '{
    "title": "打ち合わせ",
    "start": "2026-08-26T20:00:00+09:00",
    "duration_minutes": 60,
    "content": "議題: ...",
    "created_by": "agent-name"
  }'
```

`verify_availability`（既定 true）が有効なら、登録前に空きを再確認し、
埋まっていれば 409 と代替候補を返す。

### 登録先は受信トレイ（Inbox）

`project_id` / `TICKTICK_PROJECT_ID` を指定しなければ受信トレイに入る。

TickTick の受信トレイは `GET /open/v1/project` の一覧に現れず ID を引く手段が無い
（実際のIDは `inbox` + 数字）。そのため **projectId を送らないこと** が Inbox 指定の方法になる。
取り消しには projectId が要るので、作成レスポンスが返した値を保存している。

### `DELETE /v1/reservations/{id}`

### 既知のTickTick API不具合（2026-08 時点）

**DELETE が 200 を返すのにタスクが消えない。** 実機で確認済み（繰り返しても消えない）。
そのため取り消しは `complete`（完了扱い）を主経路とし、DELETE は併用に留めている。
完了済みタスクはカレンダー上で時間を占有しないため、取り消しとしては十分に機能する。

### 同期遅延への対策

TickTick → Google Calendar の同期には遅延がありうるため、本APIが登録した予約は
`reservations` テーブルにも記録し、空き判定時に FreeBusy とマージする。
これにより「登録直後に同じ枠を空きと案内する」二重予約を防ぐ。

実測では同期はほぼ即時で、2件目は `busy_calendar` として弾かれた。
DBの記録は保険として機能している。
