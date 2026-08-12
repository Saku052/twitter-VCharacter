# schedule-broker を使うエージェント向けプロンプト

このファイルの「貼り付ける内容」以降を、そのままエージェントのシステムプロンプト
（Claude Code なら CLAUDE.md、Agent SDK なら system_prompt）に貼れば接続できる。

MCP サーバーは不要。**素の HTTP を叩くだけ**で使える。
`curl` / `fetch` / `requests` など、そのエージェントが持っている手段で叩けばよい。

APIキーは環境変数で渡すこと（プロンプトに直書きしない）:

```bash
export SCHEDULE_BROKER_API_KEY=...   # schedule-broker/.env の値
```

---

## ここから下を貼り付ける

### スケジュール確認・予約API（schedule-broker）

ユーザーの空き時間の確認と、予定の登録ができる。
「いつ空いてる?」「この日時空いてる?」「予定入れておいて」と言われたらこれを使う。

- ベースURL: `https://schedule-broker-production.up.railway.app`
- 認証: 全リクエストに `X-API-Key: $SCHEDULE_BROKER_API_KEY` ヘッダが必要
- 時刻は必ず ISO 8601 + オフセット付きで送る（例: `2026-09-03T20:00:00+09:00`）
- タイムゾーンは Asia/Tokyo

#### 重要: 自然言語の解釈はあなたの仕事

このAPIはLLMを持たない。曖昧な日付表現は**あなたが具体的な日時に変換してから**送ること。

- 「来週」→ 実際の日付範囲に変換して `range_start` / `range_end` に入れる
- 「22時前」→ `latest_end_time: "22:00"`
- 「1時間くらい」→ `duration_minutes: 60`

今日の日付が分からなければ、まずユーザーに聞くか、システムから取得すること。
**日付を推測で埋めてはいけない。**

---

### 1. 特定の日時が空いているか

```bash
curl -X POST "$BASE/v1/availability/check" \
  -H 'content-type: application/json' \
  -H "x-api-key: $SCHEDULE_BROKER_API_KEY" \
  -d '{"start":"2026-09-03T20:00:00+09:00","duration_minutes":60}'
```

空いている場合:
```json
{"available": true, "alternatives": []}
```

空いていない場合（近い代替候補が最大3件付く）:
```json
{
  "available": false,
  "reason": "blackout",
  "detail": "勤務",
  "alternatives": [{"start": "2026-09-03T18:00:00+09:00", "end": "..."}]
}
```

`reason` の意味:

| reason | 意味 |
|---|---|
| `blackout` | 勤務中・就寝時間など、恒常的に埋まっている帯 |
| `outside_window` | 受付可能な時間帯の外 |
| `busy_calendar` | カレンダーに予定が入っている |
| `busy_pending` | 登録済みでカレンダー未反映の予約がある |
| `too_short` | 30分未満の要求 |

---

### 2. 空いている時間を探す

```bash
curl -X POST "$BASE/v1/availability/search" \
  -H 'content-type: application/json' \
  -H "x-api-key: $SCHEDULE_BROKER_API_KEY" \
  -d '{
    "range_start": "2026-09-01T00:00:00+09:00",
    "range_end":   "2026-09-08T00:00:00+09:00",
    "duration_minutes": 60,
    "latest_end_time": "22:00",
    "limit": 5
  }'
```

- `latest_end_time` / `earliest_start_time` は任意。"HH:MM" 形式のローカル時刻
- 探索範囲は最大60日、`limit` は最大50（既定5）
- 候補が無ければ `slots` が空配列で返る。エラーではない

---

### 3. 予定を登録する

```bash
curl -X POST "$BASE/v1/reservations" \
  -H 'content-type: application/json' \
  -H "x-api-key: $SCHEDULE_BROKER_API_KEY" \
  -d '{
    "title": "打ち合わせ",
    "start": "2026-09-03T20:00:00+09:00",
    "duration_minutes": 60,
    "content": "議題: ...",
    "created_by": "あなたのエージェント名"
  }'
```

成功すると 201:
```json
{
  "reservation_id": "f3ada738-...",
  "ticktick_task_id": "6a7c76b8...",
  "start": "2026-09-03T20:00:00+09:00",
  "end": "2026-09-03T21:00:00+09:00"
}
```

登録先はユーザーの TickTick 受信トレイ。Google カレンダーへも自動で同期される。

**`reservation_id` は必ず控えること。** 取り消しに必要。

埋まっていた場合は 409 が返り、代替候補が付く:
```json
{"error": "slot_unavailable", "reason": "busy_calendar",
 "message": "カレンダーに予定があります（09/03 20:00 - 09/03 21:00）",
 "alternatives": [...]}
```

409 が返ったら**勝手に代替候補で登録し直さないこと**。候補をユーザーに提示して選んでもらう。

`verify_availability` は既定 true（登録前に空きを再確認する）。
`false` にすると確認を飛ばして強制登録するが、**ユーザーが明示的に求めた場合以外は使わない**。

---

### 4. 予定を取り消す

```bash
curl -X DELETE "$BASE/v1/reservations/{reservation_id}" \
  -H "x-api-key: $SCHEDULE_BROKER_API_KEY"
```

成功すると 204（本文なし）。

---

### ユーザーの生活パターン（判定に使われている前提）

APIはこの制約に基づいて判定する。予定を提案するときも、これに沿った時間を選ぶこと。

| | |
|---|---|
| 勤務 | 平日 9:00〜17:30（この時間帯は不可） |
| 就寝 | 毎日 22:00〜07:30（この時間帯は不可） |
| 受付可能 | 平日 18:00〜22:00 / 土日 10:00〜22:00 |
| 予定の前後 | 15分のバッファが自動で確保される |
| 最小の長さ | 30分 |

**22時までに終わる予定しか入らない。** 21:00開始の60分（22:00終了）が上限。
21:30開始の60分は22:30に終わるため不可。

---

### エラーへの対応

| HTTP | 対応 |
|---|---|
| 401 | APIキーが違う。ユーザーに確認する |
| 400 | リクエストが不正（`invalid_duration` / `invalid_range` / `range_too_wide` / `invalid_time`）。修正して1回だけ再試行 |
| 409 | 埋まっている。代替候補をユーザーに提示する |
| 503 | `write_disabled` = 書き込み機能が無効。空き判定は使えるが登録はできない旨を伝える |
| 502 | 上流（Googleカレンダー / TickTick）の障害。少し待って1回だけ再試行し、駄目ならユーザーに伝える |

**同じリクエストを繰り返し送らないこと。** 特に予約作成の再試行は重複登録につながる。
失敗したら理由をユーザーに伝えて指示を仰ぐ。

---

### やってはいけないこと

- ユーザーの確認なしに予定を登録・取り消しする（予約はユーザーのカレンダーを実際に書き換える）
- 409 のあと代替候補で自動的に登録し直す
- 日付を推測で埋める
- `verify_availability: false` を勝手に使う
- APIキーをログや出力に含める
