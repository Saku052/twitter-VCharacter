# PROGRESS.md

各コンポーネントの現在地と次にやることを記録する。詳細な設計の前提は `CLAUDE.md` を参照。

## twitter-VCharacter

- メモ取得 → AI生成 → X投稿 → `mark_used_memo` で使用済みマークの一連の流れは実装済み
- Railwayにデプロイ済み（`RAILPACK_RUST_VERSION=stable`, `DATABASE_URL`設定済み）

### 残課題
- Twitter投稿で401 Unauthorizedが出ることがある（APIキー権限の確認が必要）
- `memo_mq.memo` カラムにNOT NULL制約をつけるか検討（現在は`Option<String>` + `unwrap_or_default()`で対応）

## Phase3.5（実装完了）

**ゴール**: 現状`main.rs`で「メモ→ツイート文（本文+ハッシュタグ）」を1回のAI呼び出しで生成している構造を、「メモ→本文」「メモ→タグ」の2回の独立した生成ステップに分離する。

**動機**: 将来の画像生成機能（メモから画像を生成するステップの追加）を見据え、「メモから複数の異なる成果物を生成する」という構造への布石。ただし今回のスコープでは画像生成自体の設計は行わず、本文・タグの2分割のみに留める（Option A採用、汎用的な生成器抽象は今回作らない。画像生成の要件が固まってから改めて検討する）。

### 決定事項（確定）

- **分割対象**: 本文生成とタグ生成の2つのみ。画像生成を見据えた汎用port設計（例: `ContentGenerator`のような抽象）は**今回は作らない**（過剰設計を避ける）
- **AI呼び出し回数**: 1回→2回に増やす。コスト・レイテンシの増加（約2倍）は許容する
- **文字数制限**: 本文は**140字以内**（従来通り、ハッシュタグは含まない）。タグは別枠（具体的な上限文字数は今回定めない、1〜2個程度を想定）
- **結合ロジックの配置**: `domain::post::prepare_post`に集約する。シグネチャを`prepare_post(content: String) -> String`から`prepare_post(body: String, tags: String) -> String`に変更し、本文・タグを結合して最終的な投稿テキストを組み立てる責務をdomain層に持たせる（`main.rs`側で直接`format!`しない）
- **タグ生成AIへの入力**: メモのみ（生成済み本文は渡さない）。本文生成とタグ生成は互いに独立し、入力が同じメモのみのため並列実行（`tokio::join!`等）が可能な設計にする

### SYSTEMプロンプト（確定）

現行の`SYS_PRPT`（本文+タグを1回で生成）を、以下の2つに分割する。

**本文生成用**:
```
<role>技術が好きな社会人1年目のエンジニア</role>
<task>渡されたメモを元に、本人視点のツイート本文を生成する</task>
<rules>
- 140字以内
- ハッシュタグは含めない（別途生成するため）
- 砕けた口語（「〜なんだよな」「〜じゃん」「〜かもしれない」など）と断定調を内容に応じて使い分け
- 絵文字は0〜2個、内容に応じて自然に配置
- 自慢や説教にならず、気づきや失敗を等身大で書く
</rules>
```

**タグ生成用**:
```
<role>技術が好きな社会人1年目のエンジニア</role>
<task>渡されたメモを元に、ツイートに付けるハッシュタグを考える</task>
<rules>
- 内容に関連するハッシュタグを1〜2個
- 「#」を付けた状態で、スペース区切りで出力する（例: #Rust #個人開発）
- 説明文や前置きは付けず、ハッシュタグの文字列のみを出力する
</rules>
```

出力形式は「AIにスペース区切りの文字列（例: `#Rust #個人開発`）をそのまま出力させる」方式を採用。Rust側で配列にパースし直す等の追加処理は行わない（プロンプトの`<rules>`で形式を固定することで担保する）。

### 実装方針（確定）

- **`AiGenerator`のシグネチャ**: 変更しない。現状の`generate(memo, model, system)`のまま、`main.rs`から本文用・タグ用のプロンプト定数を渡して2回呼び出す。専用メソッド（`generate_body`/`generate_tags`等）は追加しない。理由: 現状の設計思想（モデル名・プロンプトは`main.rs`に定数として明示し、`generate`は汎用的な実行器として保つ。data-collector側のmain.rsも同じスタイル）との一貫性を優先する
- **並列実行**: 行わない。`tokio::join!`は使わず、本文生成→タグ生成の順に逐次`.await`する。理由: 1日1回のバッチ処理であり数秒のレイテンシ差は運用上問題にならない。並列化のコード複雑化コストに見合わない（タスクが要求する以上の抽象化をしない方針）

```rust
// main.rsでの呼び出しイメージ
let body = generator.generate(&memo, GPT_MODEL, BODY_SYS_PRPT).await.expect("本文生成に失敗しました");
let tags = generator.generate(&memo, GPT_MODEL, TAG_SYS_PRPT).await.expect("タグ生成に失敗しました");
let post = prepare_post(body, tags);
```

### 実装内容（完了）

- `ports::ai_generator::AiGenerator::generate`の引数順を`(memo, system, model)`→`(memo, model, system)`に修正し、`adapters::openai::OpenAiClient`の実装側と一致させた。実装前は両者が食い違ったままコンパイルが通ってしまっており（Rustはtraitの引数名を型チェックしないため）、`main.rs`が偶然実装側の順序で呼んでいたために事故なく動いていた危険な状態だった
- `domain::post::prepare_post`を`(content: String) -> String`（素通し）から`(body: String, tags: String) -> String`（`format!("{}\n\n{}", body, tags)`で結合）に変更
- `main.rs`の`SYS_PRPT`を`BODY_SYS_PRPT`/`TAG_SYS_PRPT`の2定数に分割し、`generate`を本文用・タグ用で2回逐次`.await`呼び出しする構成に変更
- `HANDOFF_PHASE3.5.md`は実装完了に伴い削除済み

### 動作確認（完了）

`cargo check`通過を確認後、`memo_mq`の未使用メモ1件を使って実際に`cargo run`を実行。OpenAI APIが本文生成・タグ生成で2回呼ばれ、本文（140字以内）とタグ（`#エンジニア #運用`のような「#」始まりスペース区切り形式）が別々に生成され、`prepare_post`で結合された投稿がXへの実投稿まで成功することを確認した。`mark_used_memo`まで完走し「完了！」ログを確認済み。

### タグの構造化（Vec化）（実装完了）

**背景**: タグ分析（「どんなタグがいいのか」の集計）に使いたいというニーズを受け、タグを1本の文字列ではなく構造化データ（配列）として扱う方向に変更。DBスキーマ変更（`post_tags`正規化テーブル等）は将来課題としてスコープ外、今回はAI出力〜投稿直前までのデータ構造をVec化するのみ。

**実装内容**:
- `TAG_SYS_PRPT`を「#なし・カンマ区切りで出力する」指示に変更（例: `#Rust #個人開発` → `Rust,個人開発`）
- `domain::post::parse_tags(raw: &str) -> Vec<String>`を新設。`split(',')` → 各要素`trim` → 先頭`#`を`trim_start_matches('#')`で防御的に除去（fine-tunedモデルが旧仕様の`#`付きの癖を引きずるケースへの対策）→ 空要素除去、の順で処理
- `domain::post::prepare_post`のシグネチャを`(body: String, tags: String) -> String`から`(body: String, tags: Vec<String>) -> String`に変更。`#`の付与（`format!("#{}", tag)`）と結合を`prepare_post`側の責務に集約
- `main.rs`: `generate`でタグの生文字列を取得後、`parse_tags`でパースしてから`prepare_post`に渡すフローに変更
- `post.rs`の既存テスト2件を`Vec<String>`版に更新し、`parse_tags`の単体テスト4件（通常ケース・連続カンマ・末尾カンマ・`#`混入）を追加（`cargo test`で計6件パス確認済み）
- `HANDOFF_TAG_ANALYSIS.md`は実装完了に伴い削除済み

**動作確認（完了）**: `cargo check`・`cargo test`通過後、`memo_mq`の未使用メモ1件で実際に`cargo run`を実行。タグAIの出力が正しくパースされ`#転職 #年齢`のような二重`#`なしの形式でXへ実投稿されることを確認。本文+タグの合計文字数も140字を大きく下回っており、今回の変更による文字数超過の顕在化は無し。

**スコープ外・将来課題として明示**:
- DBスキーマ変更全般（`post_tags`正規化テーブル、`memo_mq`への配列カラム追加など）
- タグの表記ゆれ正規化（`Rust`/`rust`/`RUST`等の統一）

### 残課題

- **本文+タグ結合後の140字超過チェックが存在しない**: `BODY_SYS_PRPT`は本文単体で140字以内を指示しているが、`prepare_post`で結合した最終テキスト全体の文字数チェックは無い。タグVec化により「AIが#込みで文字数調整する」前提が崩れた分、理論上は超過しやすくなる方向に働く懸念があるが、実装後の確認では顕在化していない
- **タグが0件の場合、投稿末尾に不自然な空行が残る**: `parse_tags`が空`Vec`を返すと`prepare_post`は本文の後に空行だけが付いた投稿を組み立てる。エラー化や空行除去は未対応
- 画像生成機能の設計は次フェーズで改めて検討

## Phase4（実装完了）

**ゴール**: 一部のツイートに画像を添付する機能を追加する。Phase3.5で本文/タグ生成を分離したのは、この画像生成を見据えた布石だった。

### 決定事項（確定）

- **適用範囲**: 全投稿ではなく、一部の投稿のみ画像付き（ランダム条件、一定確率）。確率は環境変数で調整可能にする。目安3割前後（仮）
- **生成タイミング**: producer側（data-collector）での事前生成ではなく、**consumer側（twitter-VCharacter）で都度判定・生成**する。ランダム条件はメモ内容やsourceに依存しないため、使われないメモに事前生成コストをかけるのは非効率という判断
- **画像生成AI**: OpenAI `gpt-image-2`。既存の`OPENAI_API_KEY`をそのまま流用できる（新規契約不要）
- **画像の方向性**: 「メモ内容を簡潔な図解・インフォグラフィックにまとめる方向（diagram）」「メモの雰囲気を表すシンプルなイラスト（simple）」の**両方を選択肢として持つ**。`gpt-image-2`ではdiagram方向でも日本語テキストが正確に描画されることを確認済み（後述のPoC結果）。どちらを使うか（都度ランダム/固定/メモ内容依存）は実装時に詰める
- **画像生成失敗時の挙動**: 画像生成・Xへのアップロードのどちらが失敗しても、**画像なしテキストのみで投稿を続行**する（その回は画像添付をスキップするだけで、投稿自体は中断しない）。Phase1〜3で確立した「1機能の失敗が全体を止めない」方針をそのまま踏襲
- **Xメディアアップロード実装**: 既存の`TwitterClient`（OAuth 1.0a自前実装、[twitter.rs](twitter-VCharacter/src/adapters/twitter.rs)）を拡張する方針。新規クレートへの切り替えは行わない。`POST https://api.x.com/2/media/upload`を呼び出す実装を追加する必要がある
- **port設計**: 新規に`ImageGenerator` trait（`generate_image(prompt: &str) -> Result<Vec<u8>>`のようなシグネチャ）を切る。既存`AiGenerator`（テキスト生成、`String`を返す）とは戻り値の型が根本的に異なるため、ISPに従い分離する。`OpenAiClient`が両方のtraitを実装する形も可
- **画像生成プロンプトの入力**: 元のメモ（`memo_text`）をそのまま使う。生成済みの本文（body）を経由しない。body/tags生成と同じ「メモ→独立した成果物」という並びに揃える。「本文と画像が意味的に一体となった投稿」という将来像はあるが、今回のスコープでは扱わない（将来課題として明示）

### PoC結果（実施済み、2026-07-20）

Rustに組み込む前に、Pythonの検証スクリプトでOpenAI画像生成API・X APIを直接呼び出し、`memo_mq`の実メモ2件（ID99: YouTube由来「ClaudeCode」関連、ID104: Qiita由来「マルチエージェント手法」関連）を使ってプロンプト品質・API疎通を検証した。

**画像生成（`gpt-image-1` vs `gpt-image-2`）**:
- `gpt-image-1`ではsimple方向（雰囲気イラスト）は2件とも良好だったが、diagram方向（図解）は2件とも実用不可。本文相当の日本語テキストが「力齡た齡佳」「白カ〇得雅に鹿り人れだい」のように文字化け・崩壊した
- 同じOpenAI API・同じ`OPENAI_API_KEY`のまま、モデル名を`gpt-image-2`に変えるだけでdiagram方向を再検証したところ、2件とも**日本語テキストが完全に正確に描画**され、タイトル・3ステップの図解・説明文まで崩れなく生成された。実用レベルと判断し、diagram方向も選択肢に復帰させた

**X API疎通確認**:
- `POST https://api.x.com/2/media/upload`が、既存のOAuth 1.0a資格情報・現行のXプランで追加設定なしに200で成功することを確認済み（`media_category: tweet_image`指定、Python `requests_oauthlib`で検証）
- 取得した`media_id`を使い、`POST https://api.x.com/2/tweets`に`media.media_ids`を含めて**本番アカウントで実際に画像付きツイート投稿し、201 Createdで成功**することを確認済み

PoCで生成した画像は[scratch/phase4_poc/](scratch/phase4_poc/)に保存済み（`.gitignore`で`scratch/`除外設定済み、リポジトリには含まれない）。

### 実装時に検証・確定が必要な事項

- **既存のOAuth 1.0a自前実装（[twitter.rs](twitter-VCharacter/src/adapters/twitter.rs)）が`media/upload`のmultipartリクエストにそのまま対応できるか未検証**。PoCでの疎通確認は別ライブラリ（Python `requests_oauthlib`）で行っており、既存の自前HMAC-SHA256署名ロジック（クエリ・ボディを署名対象に含まない簡易実装）がmultipart form-dataでも同様に通るかは、Rust実装時に確認が必要
- 画像付き確率の環境変数名・デフォルト値（例: `IMAGE_POST_PROBABILITY`）
- `IMAGE_SYS_PRPT`（画像生成用プロンプト）の具体的な文面、および simple/diagram の使い分けロジック
- `ports::text_publisher::TextPublisher::post_text(content: &str)`は画像を扱えないため、シグネチャ変更または新規メソッド追加が必要（例: `post_text_with_image(content: &str, image: Vec<u8>)`）。既存の`post_text`のみの呼び出し元（`main.rs`）との整合を取る設計が必要
- `gpt-image-2`の`quality`/`size`パラメータの本番設定値（PoCでは`quality=low, size=1024x1024`を使用）
- `config::build_app()`統合方法（戻り値タプルへの`ImageGenerator`追加）と関連環境変数

### スコープ外・将来課題として明示

- 「本文と画像が意味的に一体」となるような生成フロー（本文→画像の逐次生成等）。今回は本文生成と画像生成を完全に独立させる
- 既存の残課題（本文+タグ結合後の140字超過チェック、タグ0件時の空行）— Phase4のスコープには含めない
- 障害通知・アラートの仕組み（Phase1から継続の未解決事項）

### 参考: 調査済みの技術情報

- OpenAI画像生成APIは`POST https://api.openai.com/v1/images/generations`。GPT画像モデル（`gpt-image-1`/`gpt-image-2`系）は常にbase64（`b64_json`）でレスポンスを返す（`url`は返らない）
- X側は投稿エンドポイント（`POST /2/tweets`）とは別に、`POST /2/media/upload`で先に画像をアップロードして`media_id`を取得し、投稿時に`media.media_ids`として添付する2段階構成が必要。1投稿に画像は最大4枚まで添付可能

### 実装内容（完了）

`HANDOFF_PHASE4.md`の指令書通りに実装。指令書は実装完了に伴い削除済み。

- **port設計**: `ImageGenerator`（`ports/image_generator.rs`、`generate_image(prompt: &str) -> Result<Vec<u8>>`）と`MediaUploader`（`ports/media_uploader.rs`、`upload_media(image: &[u8]) -> Result<String>`）を新規に分離。「画像バイナリの生成」と「Xへのアップロード」は責務が異なるため、同じtraitにまとめなかった
- **`OpenAiClient`**（[adapters/openai.rs](twitter-VCharacter/src/adapters/openai.rs)）: `ImageGenerator`を追加実装。`gpt-image-2`固定、`size=1024x1024`, `quality=low`、レスポンスの`b64_json`を`base64`crateでデコードして返す
- **`TwitterClient`**（[adapters/twitter.rs](twitter-VCharacter/src/adapters/twitter.rs)）: `MediaUploader`を追加実装。`POST https://api.x.com/2/media/upload`にmultipart/form-data（`reqwest::multipart`）で送信し`media_id`を取得。既存の`build_oauth_header`（URLのみを署名対象とする簡易実装）をそのまま流用し、追加対応なしで動作した
- **`TextPublisher::post_text`**: シグネチャを`post_text(content: &str, media_ids: Option<Vec<String>>)`に変更。`media_ids`がある場合はJSONボディに`media.media_ids`を追加
- **`config::build_app()`の戻り値型**: 指令書は4要素タプル（`ImageGenerator`実装を別枠で追加、`Clone`が必要になる想定）を提案していたが、`OpenAiClient`が`AiGenerator + ImageGenerator`を、`TwitterClient`が`TextPublisher + MediaUploader`を同一インスタンスで実装しているため、既存の3要素タプルのまま各要素の型を`impl TraitA + TraitB`にまとめる形にした。`Clone`導出は不要
- **`main.rs`**: `IMAGE_POST_PROBABILITY`（デフォルト0.3）で確率判定→該当すれば`generate_image`→`upload_media`、いずれかが失敗すれば`None`にフォールバックしテキストのみで`post_text`を呼ぶ
- **画像プロンプト**: 指令書5章のsimple方向テンプレートを`IMAGE_PROMPT_TEMPLATE`定数として`main.rs`に固定採用（`{memo}`をメモ本文で置換）。diagram方向は今回実装せず、使い分けロジックも設けていない（指令書1章の「迷ったら固定から始めてよい」を採用）

### 動作確認結果（本番アカウントで実施済み）

`cargo check` / `cargo test`（既存6件）通過を確認後、`memo_mq`の未使用メモを使って`cargo run`を3パターン実行。

1. `IMAGE_POST_PROBABILITY=1.0`: 画像生成→アップロード→画像付きツイート投稿が成功（「投稿成功」ログ、X API 201相当のレスポンス）
2. `IMAGE_POST_PROBABILITY=0.0`: 画像処理をスキップし、従来通りテキストのみの投稿が成功
3. 画像生成失敗ケース: `OPENAI_IMAGE_API_URL`を一時的に無効なURLに差し替えて意図的に404を発生させ、「画像生成失敗、テキストのみで続行」のログ出力後、テキストのみで投稿が最後まで成功することを確認（確認後は元のURLに戻し`cargo check`再通過を確認済み）

なお、失敗時フォールバックの検証は環境変数の差し替え（`OPENAI_API_KEY`や`TWITTER_API_KEY`を無効化する方式）では成立しなかった。`OPENAI_API_KEY`は本文生成にも使われるため無効化すると本文生成の`.expect()`でパニックし、`TWITTER_API_KEY`は投稿本体の認証にも使われるため無効化すると画像アップロード失敗後の投稿自体も401で失敗する。画像生成エンドポイントのURLのみを一時的に壊す方法で、image生成だけの単独失敗を切り分けて確認した。

### 残課題

- OAuth 1.0a署名のmultipart対応は「追加対応不要でそのまま動いた」ことを実測確認したのみで、署名ベース文字列の理論的な正しさ（RFC 5849上multipartボディを署名対象外とする扱いで一貫しているか等）を厳密に検証したわけではない。現状の簡易署名実装（クエリ・ボディ非対応）に起因する将来的な別エンドポイント対応時は都度実測での確認が必要
- simple/diagram方向の使い分けロジックは実装していない。simple方向に固定し、diagram方向は未実装（指令書1章で許容された進め方）
- 既存の残課題（本文+タグ結合後の140字超過チェック、タグ0件時の空行、障害通知の仕組み）は今回も対応なし（スコープ外として明示済み）

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

## Phase2（実装完了）

**ゴール**: YouTube以外のデータソースを追加し、かつ人間が事前にリスト（プレイリストやキーワード）を用意しなくても「今の業界トレンド」を自動的に拾えるproducerを増やす。

### 検討の経緯（X/Twitterを見送った理由）

当初はX（Twitter）をデータソースにする案を検討したが、調査の結果2つの制約が判明し、当初のゴールと相性が悪いと判断した。

1. **X Premium（個人向けサブスク）とX API利用料は完全に別課金体系**。Premium加入によるAPI無料枠・割引は存在しない。X API自体は2026年2月以降、完全従量課金（Pay-per-use）に一本化されている（Post読み取り $0.005/件、投稿作成 $0.015/件）
2. **公式Trends API（`GET /2/trends/by/woeid`）はLegacy Pro以上（月$5,000〜）が必須で新規契約不可**。「キーワードなしでトピックを自動発見する」を実現できる唯一のX公式手段が、個人開発の予算では事実上使えない
3. 代替として検討したキーワード検索（`GET /2/tweets/search/recent`）は、`min_faves`等のエンゲージメントフィルタがAPI v2では機能せず、人気順ソートも提供されない。かつ「何を検索するか」というキーワード選定自体が、結局YouTubeプレイリストと同じ「人間が事前にリストを用意する」構造に戻ってしまい、「キーワードすら要らない自動発見」というゴールを満たせない

コスト試算（参考、キーワード検索を使う場合）: 取得件数×$0.005。100件/週なら月$2程度、300件/週でも月$6程度と、コスト自体は小さいが、上記2の理由でそもそもの目的を達成できないため見送り。

### 現在の方向性（決定）

**Qiita API v2 を新規producerとして追加する。**

- 公式API・無料・認証不要でも利用可能
- `stocks_count`（ストック数）等の人気指標がレスポンスに標準で含まれ、`query=stocks:>N` のようなクエリで人気記事に絞り込める → X検索で直面した「フィルタできない」問題が構造的に発生しない
- 日本語の技術記事が中心で、「さく」（社会人1年目エンジニア）というキャラクター設定と相性が良い
- キーワードによる絞り込みではなく、ストック数上位を機械的に取得する形にすれば、YouTubeプレイリストのような「事前リスト管理」を回避できる（今回のゴールに合致）

**見送った代替候補**: Hacker News API（公式・無料・`score`順取得可能だが海外中心で「さく」との相性がQiitaより弱い）、Zenn（非公式APIのみで公式ドキュメントなし、依存リスクが高い）。将来的にソースを増やす際の次点候補として記録。

### Qiita API仕様調査結果

実際にQiita API v2を呼び出して検証済み（2026-07-19時点の実データ）。

- `id`フィールドが記事の一意ID。重複排除のキーとして利用可能
- `query`パラメータで`created:>日付`・`updated:>日付`・`stocks:>N`・`stocks:<N`の範囲指定が可能。ANDで複数条件を並べられる
- レート制限: 未認証60req/h、認証あり1,000req/h。1日1回・少量アクセスなら未認証でも十分
- **増加速度（トレンドの勢い）はAPIから直接取得不可**。返るのは常にその時点のスナップショット値のみ。差分計算には過去の取得結果を自前で保存・比較する必要があり、コストに見合わないため見送り
- 実測: `stocks:>20 stocks:<50 created:>直近7日`で15件、`stocks:>100`は2件のみ（母数が少なすぎる）、`stocks:>200`は0件

### 決定事項（確定）

- **クエリ**: `stocks:>20 stocks:<50 created:>{直近1日}` で取得。上限を切ることで、個人の突出した実績報告記事（バズった自作サービス紹介等）を自然に除外できる副次効果もある
- **`created`基準**（`updated`は不採用。誤字修正等の些細な編集でもヒットしてしまい、「今起きているトレンド」という意図からズレるため）
- 増加速度の取得・判定は行わない（スコープアウト）
- 他人の実績報告記事のフィルタリング（AI判定等での除外）は**スコープアウト**。stocks上限による間接的な軽減のみで妥協する
- 取得頻度: **1日1回**（YouTube側と同じデイリーバッチ）
- Qiita API認証: **一旦トークンなしで進める**。1日1回・少量アクセスのため未認証の60req/hで足りる想定。将来詰まったら認証を追加
- YouTube producerとQiita producerは**同一`main.rs`内で直列実行**する（別バイナリ・別cronに分けない）。Hexagonal Architectureの構成上、両方とも同じportの形（`fetch_recent_videos`的なメソッド）に揃えられるため、逐次呼び出すだけで自然に合成できる
- 重複排除テーブルは`processed_videos`とは**別テーブル**にする（`processed_qiita_items(article_id TEXT PRIMARY KEY, processed_at TIMESTAMPTZ)`）。IDの意味が異なるソース間でテーブルを共有しない方針は[ARCHITECTURE.md §3.3](ARCHITECTURE.md#33-共有dbテーブル)の設計方針を踏襲
- `memo_mq`に**`source TEXT`列を追加**する（`ALTER TABLE memo_mq ADD COLUMN source TEXT`）。値は`'youtube'` / `'qiita'`を想定。デバッグ用に、どのproducer由来のメモか後から追えるようにする。既存行は`NULL`のままで後方互換は保たれる
- AIに渡す記事本文は**先頭300文字程度**を使う（タイトルのみだと情報不足、全文だとAI入力コストが増えすぎるため折衷）

### data-collector側 SYSTEMプロンプト改訂（確定）

YouTube専用だった現行プロンプトを、YouTube/Qiita両対応に変更する。

**変更点**
1. `<role>`: `技術が好きな社会人1年目のエンジニアのメモ係` → `さく担当の編集者。ネタ元を読んで、さくが話したくなりそうなポイントだけ抜き出す`
   - 「編集者」という第三者性を明示し、生成物が常に「誰かの情報の要約」であるという前提を強め、後段（twitter-VCharacter側）で一人称の体験談に変換されてしまうリスクを上流で予防する狙い
2. `<task>`: 情報源をソース非依存の表現に変更し、`[YouTube動画]`または`[Qiita記事]`のタグをAIへの入力に含める（呼び出し側で`format!("[{}] タイトル: {}\n{}", source_label, title, body_excerpt)`のようにラベル付けする）
3. `<rules>`: 「コードや型名など込み入った専門用語は、メモの時点で噛み砕く（ツイート生成側では元情報を参照できないため）」を追加

**改訂後の全文**
```
<role>さく担当の編集者。ネタ元を読んで、さくが話したくなりそうなポイントだけ抜き出す</role>
<task>渡された情報（[YouTube動画]または[Qiita記事]）を読み、ネタにできそうな『気づき』『学び』『感想の種』を日本語の短いメモとして1個出力する</task>
<rules>
- ハッシュタグや絵文字は付けない
- 事実をそのまま要約するのではなく『これ面白いな』『これ自分でも試したい』のような感想・気づきの形に変換する
- 専門用語は無理に避けず、ただし社会人1年目が背伸びしすぎない温度感で
- コードや型名など込み入った専門用語は、メモの時点で噛み砕く（ツイート生成側では元情報を参照できないため）
- 50文字以内
</rules>
```

**スコープアウトした案**: メモに「〜という記事を見た」のような伝聞ニュアンスを持たせるルール（情報源の言明）は今回見送り。role変更のみで対応する。

### 未決事項（保留中）

- twitter-VCharacter側の「本人視点のツイートを生成する」プロンプト自体の変更可否。data-collector側のrole/rules改訂で上流の予防線は張ったが、2段階目（ツイート生成）での変換リスクが完全に解消したわけではない。ユーザーが後日改めて確認予定

### 実装内容（完了）

- `domain::QiitaArticle { article_id, title, body_excerpt }` 新設
- `ports::qiita_port::QiitaPort::fetch_trending_articles()` と `adapters::qiita::QiitaClient`（認証なし、`query=stocks:>20 stocks:<50 created:>前日`、`per_page=100`）
  - `body`の先頭300文字切り出しは`chars().take(300)`でUTF-8文字境界を考慮（バイトスライスでの日本語パニックを回避）
  - 日付計算に`chrono` crateを追加
- `ports::memo_writer::MemoWriter`に`insert_qiita_memo` / `is_qiita_processed`を追加方式で拡張（既存の`insert_memo` / `is_processed`は破壊的変更なし）
- `adapters::postgres::PostgresClient`に上記2メソッドを実装。`processed_qiita_items`テーブルへの重複排除書き込みと、`memo_mq`への`source`列付き書き込み（`'youtube'` / `'qiita'`）を同一トランザクションで実施
- `config::build_app()`の戻り値タプル末尾に`QiitaPort`を追加（`(YoutubePort, AiGenerator, MemoWriter, QiitaPort)`、`app.0`〜`app.2`の参照は不変）
- `main.rs`: YouTubeループの後にQiitaループを直列追加。`success_count`/`failure_count`は両ソース共通カウンタとして合算判定。SYSTEMプロンプトを「編集者」視点・ソース非依存の内容に改訂（[YouTube動画]/[Qiita記事]タグ付き入力に対応）
- DBスキーマ変更（`processed_qiita_items`作成、`memo_mq.source TEXT`列追加）をRailway DBに対して実行済み
- `cargo sqlx prepare`実行、`.sqlx/`キャッシュ更新済み
- 動作確認済み: 1回目実行でYouTube・Qiita双方が`memo_mq`に`source`列付きで書き込まれることをDB上で確認、2回目実行で両ソースとも重複排除により全件スキップされ正常終了（exit code 0）することを確認

### 残課題（Phase2完了後）

- Railway側のcronスケジュール登録（1日1回、YouTube/Qiita両方カバー）はまだ未実施
- Qiitaクエリ（`stocks:>20 stocks:<50`）は実測でヒット数が少なめ（1日0〜数件程度）なので、しばらく運用してヒット数の傾向を見た方がよい
- `main.rs`のYouTube/Qiitaループはほぼ同一構造の重複コード。ソースが3つ目に増える際は共通処理への切り出しを検討

### レビュー指摘への対応（完了）

チームリーダーからの指摘: `fetch_recent_videos()` / `fetch_trending_articles()`自体の取得失敗が`.expect()`（panic）のままで、Phase1のYouTube側の設計をQiitaにも複製していた。片方のソースのAPI障害でプロセス全体が落ち、もう片方が正常でもバッチ全体が異常終了・ログも途切れる問題があった。

対応: 両方とも`match`でエラーを捕捉し、失敗時は空`Vec`にフォールバック＋`failure_count`をカウントする形に修正。片方のソースが落ちてももう片方は独立して処理が継続し、最終的な合算判定（`failure_count > 0 && success_count == 0`）にも正しく反映される。

## Phase3（要件検討中・未着手）

**ゴール**: YouTube/Qiitaのような固定ソース・固定クエリのproducerではなく、Agent SDK（汎用Web検索ツールを持たせたAIエージェント）に「今日の技術トレンドを調べて」のような抽象的な指示だけを与え、ソース選定・調査自体を自律的に行わせる。

### Phase1/2との根本的な違い

Phase1/2は「決まったAPIから決まったクエリでデータを取得する」静的producerだった。Phase3はアーキテクチャ的に一段階異なり、以下の前提が崩れる、または再検討が必要になる。

| Phase1/2の前提 | Phase3で崩れる理由 |
|---|---|
| `YoutubePort` / `QiitaPort`のようにソースごとに固定trait・adapterを用意 | エージェントがソースを動的に選ぶため、コンパイル時にソースを決め打ちできない |
| `main.rs`でYouTube→Qiitaと直列にハードコード | ソース数・種類が可変なため、この直列構造は成立しない |
| クエリ・閾値を人間が事前に決めて埋め込む（`stocks:>20 stocks:<50`等） | エージェントが自律判断するため「何を調べるか」自体を固定しない |
| 重複排除はソースごとの専用テーブル（`processed_videos`/`processed_qiita_items`） | ソースが可変なため、専用テーブルを都度作る設計は成立しない。汎用的な仕組みが必要（未検討） |
| コストが読みやすい（API呼び出し回数が固定） | エージェントのループ回数・検索回数が実行毎に変動しうる。青天井のコストリスク |

### 決定事項（確定）

- **検索ツール**: 汎用Web検索API（Google/Bing系）を採用。特定ドメインへのホワイトリスト制限は設けない
- **信頼性担保の方針**: 事前の厳密な制御は行わない。Phase2で導入済みの`memo_mq.source`列を活用し、「Agent SDK経由」であることを明記した上で運用しながら判断する。評判が悪ければ事後的にロールバックする運用ベースの方針とする（現時点ではこれ以上詰めない）
- **重複排除**: スコープアウト。ソースが可変でURLベースの一意キーも綺麗に取れないため、Phase1/2のような専用テーブルでの重複排除は行わない
- **コスト上限**: **ターン数上限**方式を採用（Agentが「検索→思考」を繰り返せる最大回数を制限）。目安として1回のバッチにつき**10ターン**程度から開始し、運用しながら調整する。予算(金額)ベースの上限は今回は導入しない
- **実行頻度**: 1日1回
- **出力件数**: 1バッチ = **1件のメモ**（YouTube/Qiitaは5件/日だが、Agent調査は1回の調査で1件に集約する）
- **実行順序**: YouTube/Qiitaとの直列・並列関係は特に規定しない。最終的に`memo_mq`に書き込まれれば良いという考え方

### アーキテクチャ方針（確定）

Agent SDK部分はRustプロジェクト内に実装せず、**独立したPython wrapperアプリケーションとして別デプロイ**し、Rust側からHTTP経由で呼び出すHexagonal Architecture構成にする。

```
data-collector (Rust)
  └─ ports::agent_port::AgentPort (trait)
       └─ adapters::agent::AgentClient
            └─ HTTP(REST)で呼び出し → Agent SDK wrapper (Python, Railway上に別サービスとしてデプロイ)
                 └─ 内部でWeb検索ツールを使って自律調査、最終的にメモ(String)を1件返す
```

- **デプロイ先**: Railway上に、twitter-VCharacter・data-collectorと並ぶ3つ目の別サービスとして立てる
- **通信方式**: シンプルなHTTP(REST)。Python側はFastAPI等で`POST /investigate`のような1エンドポイントを公開し、Rust側は既存のreqwestでそのまま叩く（新規依存クレート不要）
- Rust側のコードにAgent SDK/Pythonの実装詳細は一切漏れない。`AgentPort` traitがHTTP呼び出しをラップする

### プロンプト設計（確定）

調査の結果、Claude Agent SDK公式ドキュメントが「CLAUDE.md（baseline/恒常ルール）+ systemPrompt append（task-specific/タスク単位の指示）をSDKがlayerとして重ねる」という構成を公式に推奨していることが判明。この2層構成を採用する。

**重要な実装上の注意点**: CLAUDE.mdはSDKが自動では読み込まない。Python側で`ClaudeAgentOptions(setting_sources=["project"])`を明示的に指定しないと、CLAUDE.mdファイルをどれだけ書いても無視される（本番デプロイでの最頻出の設定ミスとして公式ドキュメントに明記あり）。Python wrapper実装時に必ず反映すること。

**モデル**: Sonnet 5、reasoning effort high

**CLAUDE.md（恒常ルール、Web検索wrapperのプロジェクトルートに配置）**:
```markdown
# CLAUDE.md

## あなたの役割
さく（技術が好きな社会人1年目のエンジニア、VTuberキャラクター）担当の編集者です。
Web検索で見つけたネタ元を読み、さくが話したくなりそうなポイントだけを抜き出してメモにします。

## 出力ルール
- 出力は日本語の短いメモ1個のみ（50文字以内）
- ハッシュタグや絵文字は付けない
- 事実をそのまま要約するのではなく「これ面白いな」「これ自分でも試したい」のような感想・気づきの形に変換する
- 専門用語は無理に避けず、ただし社会人1年目が背伸びしすぎない温度感で
- コードや型名など込み入った専門用語は、メモの時点で噛み砕く

## 配慮事項
- 個人の具体的な実績・成果物（自作サービスの収益、バズった投稿の裏側など）を、さく自身の体験のように語らない
- 一次情報の断定ではなく、あくまで「見つけたネタを編集した」という前提を保つ
```

**タスクプロンプト（systemPrompt append、毎回の調査指示）**:
```markdown
今日の技術トレンドを1つ調べて、メモを1個作成してください。

## 手順

### Step 1: 広く探る（検索2〜3回）
まず短く広いクエリで、今どんな技術トピックが話題になっているか全体像をつかんでください。
例: 「プログラミング トレンド 2026」「AI開発 話題」など。
特定の技術・製品名を最初から狙い撃ちしないこと。

### Step 2: 絞り込む（検索最大3回）
Step 1で見つけた候補の中から、気になったものを1つ選び、内容を深掘りしてください。
複数の候補を並行して深掘りしない。1つに決めてから掘る。

### Step 3: メモを確定する
選んだトピックについて、CLAUDE.mdのルールに従ってメモを1個作成し、それを最終出力としてください。
メモが完成したら、それ以上の検索は行わないこと。

## 終了条件
- メモ1個を出力したら終了
- 合計10ターンを超えたら、その時点までの情報で必ずメモを1個確定させて終了する（探索を続けない）
- 「これ以上良いネタが見つかるかもしれない」という理由だけで探索を継続しないこと
```

設計のポイント: 各ステップに検索回数の目安を明記（Anthropic公式記事の「タスク規模に応じた予算を先に決める」原則）、「1つに決めてから掘る」を明示（並行調査による予算浪費を防ぐ）、終了条件を「メモ完成」と「10ターン超えたら強制確定」の2通りで重ねて書き、Agentが際限なく探索を続けるのを防ぐ。

### 失敗時・出力フォーマットの方針（確定）

- **出力フォーマット**: メモ文字列のみ（参照元URL等の構造化データは持たない）
- **失敗時**: Agentが終了条件までに良いメモを確定できなかった場合、Python wrapper側は**HTTPエラーを返す**（空文字列を正常応答としてRust側に渡さない。`memo_mq`に空メモが入るリスクを防ぐため）
- **Rust側の扱い**: `AgentPort`からHTTPエラーが返ってきた場合、その回の処理をスキップするだけで他のロジック（YouTube/Qiita等）は継続する。Phase1/2で確立した「1ソースの失敗が全体を止めない」パターンをそのまま踏襲する

### Python wrapper / Rustインターフェース設計（確定）

**Webフレームワーク**: FastAPI。1エンドポイントだけの小さなアプリなので最も一般的な選択。Claude Agent SDK Python版もasync前提で相性が良い。

**`AgentPort`（Rust側）**:
```rust
// ports/agent_port.rs
use async_trait::async_trait;
use anyhow::Result;

#[async_trait]
pub trait AgentPort {
    async fn investigate(&self) -> Result<String>;
}
```
YouTube/Qiitaの`Vec<T>`を返す設計とは異なり、単一の`String`（メモ文字列）を1件返す。Phase3で決めた「1バッチ=1件のメモ」を反映。Web調査は既にAgent内部でメモを作り切っているため、`main.rs`側では`is_processed`のような重複判定は挟まず、`insert_agent_memo`にそのまま渡す。

**`MemoWriter`への追加メソッド**:
```rust
async fn insert_agent_memo(&self, memo: &str) -> Result<()>;
```
YouTube用`insert_memo(memo, video_id)`・Qiita用`insert_qiita_memo(memo, article_id)`と異なり、重複排除をスコープアウトしているためID引数を取らない。`memo_mq`に`source='agent'`としてINSERTするだけのシンプルな実装になる見込み。

**認証**: 簡易認証（共有トークンを`X-API-Key`ヘッダ等に固定値で付与）。Railway内部のプライベート通信想定だが、環境変数の設定ミスで外部から叩かれる事故を防ぐため導入する。

**タイムアウト**: Rust側のHTTPクライアントは**1200秒（20分）**でタイムアウト設定する。10ターン分の検索・AI推論を含む前提で、余裕を持たせつつ異常な強直りも検知できる長さ。

**`main.rs`での呼び出しイメージ**:
```rust
match app.3.investigate().await {
    Ok(memo) => {
        match app.2.insert_agent_memo(&memo).await {
            Ok(()) => { /* success */ }
            Err(e) => { /* ログ出力、他ソースの処理は継続 */ }
        }
    }
    Err(e) => {
        eprintln!("Agent調査に失敗: {:?}", e);
        // このスキップだけで、YouTube/Qiitaの処理には影響しない
    }
}
```

### Web検索・認証まわりの調査結果（確定＋一部未決）

**Web検索ツール**: Claude Agent SDKには`WebSearch`が標準の組み込みツールとして提供されている（server tool、Anthropicのインフラ上で実行される）。`allowed_tools`に追加するだけで使え、別途Google/Bing等の検索APIキーを用意する必要はない。

**認証方式（確定）**: Claude Pro/Maxサブスクのクレジットをそのまま使う。`claude setup-token`コマンドで発行したOAuthトークン（`sk-ant-oat01-`で始まる）を`CLAUDE_CODE_OAUTH_TOKEN`環境変数にセットする方式が公式にサポートされている。Claude Proは月$20分のAgent SDKクレジットを含む。ユーザーが既に他プロジェクトで発行済みのトークンをそのまま流用する。

**実装上の重大な注意点（確定）**: Python wrapperのデプロイ環境（Railway）に`ANTHROPIC_API_KEY`環境変数を**絶対に設定しないこと**。この変数が存在すると、Claude Agent SDKはサブスクのOAuthトークンより優先してAPIキー側の従量課金ルートを使ってしまい、意図せず別課金が発生する。`CLAUDE_CODE_OAUTH_TOKEN`のみを設定する。

**未決事項として保留**: Web検索ツール（`$10/1,000検索`という従量課金情報が見つかっている）が、サブスクのAgent SDKクレジット枠に含まれるのか、それともサブスク経由でも別途課金が発生するのかは今回の調査で確認しきれなかった。結論を出さず保留し、実装後に少額のテスト実行を行って請求実績で確認する方針とする。

### `build_app()`統合・環境変数（確定）

`config::build_app()`の戻り値タプル末尾に`AgentPort`を追加する（既存の`(YoutubePort, AiGenerator, MemoWriter, QiitaPort)`を`(..., AgentPort)`に拡張、`app.0`〜`app.3`の参照は不変）。

```rust
pub async fn build_app() -> Result<(impl YoutubePort, impl AiGenerator, impl MemoWriter, impl QiitaPort, impl AgentPort)> {
    // 既存の組み立てに加えて
    let agent_client = AgentClient::new(
        env::var("AGENT_SDK_URL").expect("AGENT_SDK_URL が設定されていません"),
        env::var("AGENT_SDK_API_KEY").expect("AGENT_SDK_API_KEY が設定されていません"),
    );

    Ok((youtube_client, openai_client, memo_repo, qiita_client, agent_client))
}
```

Rust側（data-collector）に新規追加する環境変数:
- `AGENT_SDK_URL`: Python wrapperのRailway上のURL（サービス間の内部通信用）
- `AGENT_SDK_API_KEY`: 簡易認証用の共有トークン（`X-API-Key`ヘッダに付与）

### 未決事項（次回詰める）

- Web検索ツールの課金がサブスク枠内かどうか（上記、実装・テスト実行で確認）
- Railway側のデプロイ設定（Python wrapperサービスの環境変数。`CLAUDE_CODE_OAUTH_TOKEN`を設定し`ANTHROPIC_API_KEY`は設定しないことを徹底する）

## Phase3（実装完了）

`HANDOFF_PHASE3.md`の指令書通りに実装。設計の詳細は上記の各セクションを参照。

### 実装内容

- **agent-wrapper/**（新規プロジェクト、リポジトリルート直下）
  - `main.py`: FastAPI、`POST /investigate`エンドポイント1本。`X-API-Key`ヘッダで簡易認証（`AGENT_SDK_API_KEY`環境変数と照合、不一致は401）
  - `ClaudeAgentOptions(cwd=agent-wrapperディレクトリ, setting_sources=["project"], allowed_tools=["WebSearch"], max_turns=10, model="claude-sonnet-5", effort="high", system_prompt={"type":"preset","preset":"claude_code","append":タスクプロンプト})`
  - `CLAUDE.md`: 指令書通りの「編集者」ロール・出力ルール・配慮事項
  - 失敗時（`ResultMessage.is_error`が`True`、または`max_turns`到達等で例外発生）は`HTTPException(502)`を返す。空文字列/nullを200で返すことはない
  - 依存: `fastapi`, `uvicorn[standard]`, `claude-agent-sdk`（`requirements.txt`）
- **data-collector側（Rust）**
  - `ports/agent_port.rs`: `AgentPort` trait新設（`investigate() -> Result<String>`）
  - `adapters/agent.rs`: `AgentClient`（reqwest、タイムアウト1200秒、`X-API-Key`ヘッダ付きPOST、`error_for_status()`で4xx/5xxをErrに変換）
  - `ports/memo_writer.rs` / `adapters/postgres.rs`: `insert_agent_memo(memo)`追加（`source='agent'`固定、重複排除テーブルなし、トランザクション不要）
  - `config::build_app()`: 戻り値タプル末尾に`AgentClient`を追加（`(YoutubePort, AiGenerator, MemoWriter, QiitaPort, AgentPort)`、`app.0`〜`app.3`の参照は不変）。新規環境変数`AGENT_SDK_URL` / `AGENT_SDK_API_KEY`
  - `main.rs`: YouTube→Qiitaループの後にAgent呼び出しを追加。失敗しても他ソースの処理には影響しない。`success_count`/`failure_count`に合流
  - `cargo check`（オフライン含む）、`cargo sqlx prepare`実行済み・`.sqlx/`キャッシュ更新済み

### 動作確認結果（ローカル、実施済み）

ユーザーの`CLAUDE_CODE_OAUTH_TOKEN`（`claude setup-token`発行済み）を一時的にローカル環境変数として使い、agent-wrapperを`uvicorn`でローカル起動して検証。

1. 誤った`X-API-Key`で401が返ることを確認
2. 正しいキーで`POST /investigate`を実行 → 200 OKで日本語メモが返ることを確認。**1回目はAgentの最終応答に「メモを作成しました」等の前置き・見出しが混入する問題を発見**。タスクプロンプトに「出力フォーマット（重要）」セクションを追記（最後の発言＝メモ本文のみ、前置き・見出し・選定理由の説明を含めないことを明記）し修正
3. 修正後に再実行 → メモ本文のみ（例:「NVIDIA全社エンジニアがAI活用でコード量3倍なのにバグは増えてないらしくて気になる」、43文字）がクリーンに返り、CLAUDE.mdの編集者視点・感想の形のルールにも合致していることを確認
4. `max_turns=1`に強制した失敗ケースで「Reached maximum number of turns (1)」の例外が送出され、`/investigate`が502を返すことを確認（`run_investigation()`内で`ResultMessage.is_error`判定を経由）

### 残課題

- **Web検索ツールの課金がサブスク枠内かどうかの実測**: 今回のタスクではスコープ外（指令書通り）。実装は完了したので、次回はRailwayデプロイ後に少額の実行を重ねてAnthropicの請求実績で確認すること
- Railway側の3つ目のサービスとしてのデプロイ未実施（`CLAUDE_CODE_OAUTH_TOKEN`のみ設定・`ANTHROPIC_API_KEY`は絶対に設定しないことを徹底）
- Railway側cronスケジュール設定は別途（スコープ外）
- data-collector側の`.env`に`AGENT_SDK_URL` / `AGENT_SDK_API_KEY`をローカル用に追記していない（Railwayデプロイ後、ローカル開発時は本番URLかローカル起動したagent-wrapperのURLを設定する）
- `main.rs`の`total`件数カウントにAgent分の1件を加算する形にした（YouTube/Qiitaの件数ログと揃える判断）。指令書に明記がなかった箇所のため、意図と異なる場合は要調整

## QA_PRJ（実装完了）

**Phase番号を振らない独立した取り組み**。YouTube/Qiita/Agentのような新しいデータソース追加とは性質が異なり、Phase1〜3.5で実装したコード全体を横断するテスト基盤の整備。

**背景**: Phase1〜3.5が完了した時点でテストコードは0件だった（両プロジェクトとも`dev-dependencies`未設定、HTTPモックcrateも未導入）。実際に「テストがあれば検出できたはずのバグ」が2件発生していた。

1. data-collector側: YouTube APIレスポンスの並べ替えロジックの不具合（[adapters/youtube.rs](data-collector/src/adapters/youtube.rs)、修正済み）
2. twitter-VCharacter側: `ports::ai_generator::AiGenerator::generate`のtrait定義と`adapters::openai::OpenAiClient::generate`の実装の引数順不一致（Phase3.5実装時に修正済み）。Rustはtraitの引数名を型チェックしないためコンパイルは通ってしまい、`main.rs`が偶然実装側の順序で呼んでいたために事故なく動いていた

### スコープ（確定）

- **対象プロジェクト**: `data-collector`と`twitter-VCharacter`の両方
- **DB依存コード（`adapters/postgres.rs`）**: テスト対象外。`sqlx::query!`はコンパイル時に実DB接続を要求するため、目視レビュー・実運用での確認に委ねる
- **HTTPモックライブラリ**: **wiremock**を採用（`async`/`tokio`ネイティブな構成との相性を優先）

### 実装内容（完了）

- 両プロジェクトの`Cargo.toml`に`[dev-dependencies]`（`wiremock = "0.6"`, `tokio`の`test-util`feature）を追加
- `data-collector/src/adapters/youtube.rs`: `base_url`をテスト用に注入できるよう`YoutubeClient`を拡張（`#[cfg(test)] fn with_base_url`）。並べ替えロジック（`.rev().take(5)`で直近5件を新しい順に取る）のテストを追加
- `data-collector/src/adapters/qiita.rs`: 同様に`QiitaClient`へ`base_url`注入を追加。`chars().take(300)`のUTF-8文字境界処理のテスト（マルチバイト文字での非パニック確認、300字未満のケース）を追加
- `twitter-VCharacter/src/domain/post.rs`: `prepare_post`のテスト2件（タグVec化に伴い更新）、`parse_tags`のテスト4件（通常ケース・連続カンマ・末尾カンマ・`#`混入）を追加
- `cargo test`実行結果: data-collector 3 passed、twitter-VCharacter 6 passed、いずれも0 failed。DB接続・実API呼び出しなしで完結することを確認済み
- `HANDOFF_QA_PRJ.md`は実装完了に伴い削除済み

### スコープ外（今回やらないこと、次回以降の候補）

- `main.rs`内の`success_count`/`failure_count`合算ロジックのテスト（`main.rs`からロジックを切り出すリファクタリングが前提になるため見送り）
- `AiGenerator`の引数順を型レベルで守る仕組み（構造体でラップする等）の検討
- DB依存コードのテスト（Docker等でテスト用DBを用意する方式）
- CI（GitHub Actions等）へのテスト組み込み。現状CI自体が存在しないため、ローカルでの`cargo test`実行のみ
- `youtube.rs`/`qiita.rs`以外のadapter（`openai.rs`, `agent.rs`, `twitter.rs`, `postgres.rs`）のテスト追加

## Phase5（完了）

`HANDOFF_PHASE5.md`の指令書通り、「さく」のペルソナ自体を転換するタスク。「技術が好きな社会人1年目のエンジニア」から「ゲーム制作をしている個人Vtuber（社会人1年目エンジニアという背景は残す）」へ主従を入れ替えた。詳細な経緯・方針は`SNS_POSITIONING_PROPOSAL.md`参照（`HANDOFF_PHASE5.md`は完了に伴い削除済み）。

### 決定事項（確定）

- **fine-tuningは実施しない**: 当初の指令書は「プロンプト修正→検証→fine-tuning要否判断」という段階的アプローチだったが、以下の理由で方針変更した
  1. OpenAIのfine-tuning platformが2026年5月7日付で段階的縮小中と判明（7月2日以降、過去60日推論実績のない組織は新規job作成不可。2027年1月6日に既存ユーザーも完全停止）
  2. 現行の`GPT_MODEL`（`gpt-4.1`ベース）は2026年10月23日にシャットダウン予定と判明。fine-tuningするなら世代交代が必要だが、実際にOpenAIダッシュボードでbase model選択肢を確認したところ**GPT-5系はfine-tuning対象に入っていなかった**
  3. 上記を踏まえ、fine-tuning自体をやめ、素の`gpt-5.5` + プロンプトのみでペルソナを表現する方式に切り替えた
- **旧モデルID**（切り戻し用）: `ft:gpt-4.1-2025-04-14:personal:tweetsource1:DfS5fKl8`。[twitter-VCharacter/src/main.rs](twitter-VCharacter/src/main.rs)にコメントで保持
- **画像生成機能は一旦廃止**: `IMAGE_PROMPT_TEMPLATE`は「技術系VTuber」を想定した文面だったため転換対象だったが、ユーザー判断で機能自体を停止することになった。`main.rs`内の呼び出し・定数・関連importをコメントアウトで無効化（削除はしない。復活の可能性があるため）。`ImageGenerator`/`MediaUploader`のport・adapter実装はコードとして残置（dead_code警告は許容）
- **Agent SDKの`TASK_PROMPT`変更もスコープに含めた**: 指令書では別タスク扱い（§6）だったが、統合テストで「ゲーム開発Vtuberらしいメモ」が`memo_mq`に入っている状態を作る必要があったため、この場で前倒しして実施
- **Agent SDKの1回あたりメモ生成件数を1→2に変更**: ユーザー要望。`agent-wrapper`の`/investigate`レスポンス契約を`{"memo": str}`から`{"memos": [str]}`に破壊的変更し、`data-collector`側の`AgentPort::investigate()`・`AgentClient`・`main.rs`を連動して更新
- **Qiitaの`tag:ゲーム開発`絞り込みもこの場で実施**: 指令書§6では別タスク扱いだったが、ペルソナ転換の一連の作業として合わせて実装。クエリを`stocks:>20 stocks:<50 created:>5日前`から`tag:ゲーム開発 stocks:>1 created:>30日前`に変更（HANDOFF記載の実測値: `stocks:>1`・直近30日で28件を採用）
- **Qiita取得件数の上限を3件に制限**: 初回実行時、`tag:ゲーム開発`が過去に一度も重複排除されていなかったため`per_page=100`のままヒットした約28件がほぼ全件`memo_mq`に一括投入される事態が発生。1回のバッチで大量にメモが積み上がるのを避けるため、`per_page`を`3`に変更（1回の実行あたり最大3件まで取得）
- **YouTubeプレイリストの中身は未着手**: プロダクトオーナー自身が編集する想定（指令書§6通り、コード変更不要）

### 実装内容

- **`twitter-VCharacter/src/main.rs`**
  - `BODY_SYS_PRPT`/`TAG_SYS_PRPT`の`<role>`を「個人でゲームを作っているVtuber。普段は社会人1年目のエンジニアとして働いていて、その経験を活かして自分のゲームを作っている」に変更（`<rules>`は現状維持）
  - `GPT_MODEL`を`gpt-5.5`に変更（旧fine-tunedモデルIDはコメントで保持）
  - 画像生成（`IMAGE_PROMPT_TEMPLATE`定義、`main()`内の生成・アップロード呼び出し、関連import）をコメントアウト。`media_ids`は`None`固定
- **`data-collector/src/main.rs`**
  - `SYSTEM`の`<rules>`内、技術記事前提だった2行を「ゲーム開発・エンジニアリングどちらの専門用語も無理に避けず」に調整
  - Agent SDK処理部分を、複数メモ（`Vec<String>`）をループして`insert_agent_memo`する形に変更
- **`data-collector/src/adapters/qiita.rs`**: クエリを`tag:ゲーム開発 stocks:>1 created:>30日前`に変更。`per_page`を`100`→`3`に変更（1回の実行あたり最大3件までに制限）
- **`data-collector/src/ports/agent_port.rs`**: `investigate()`の戻り値を`Result<String>`→`Result<Vec<String>>`に変更
- **`data-collector/src/adapters/agent.rs`**: `InvestigateResponse`のフィールドを`memo: String`→`memos: Vec<String>`に変更（Python側の新レスポンス契約と一致）
- **`agent-wrapper/main.py`**
  - `TASK_PROMPT`: 「今日の技術トレンドを1つ」→「今日のゲーム開発関連の話題を2つ」。検索例・終了条件・出力フォーマット（改行区切り2行）も合わせて変更
  - `run_investigation()`: 戻り値を`str`→`list[str]`に変更（最後の発言を改行分割）
  - `/investigate`: レスポンスを`{"memo": ...}`→`{"memos": [...]}`に変更
- **`agent-wrapper/CLAUDE.md`**: `main.rs`側とは独立して存在していた「技術が好きな社会人1年目のエンジニア」というペルソナ記述を発見し、同じ表現に統一。`<rules>`もゲーム開発用語対応に調整

### 動作確認結果

- `twitter-VCharacter`・`data-collector`ともに`cargo check`成功（画像生成関連の`dead_code`警告のみ、想定通り）
- `agent-wrapper/main.py`のPython構文チェック成功
- **`agent-wrapper`のローカル起動確認を実施**（`uvicorn`で`127.0.0.1:8787`起動、`AGENT_SDK_API_KEY`はローカル一時値）。`POST /investigate`を実際にWeb検索込みで実行し、200 OKで`{"memos": ["ゲームで稼げなくても続ける道があるって話、心が軽くなった", "1週間で1本完成させる縛り、やることを絞る発想が新鮮だった"]}`を確認。2件・ゲーム開発文脈・50字以内のルールをいずれも満たしていた
- **`twitter-VCharacter`の`cargo run`を1回だけ実行**（ユーザー判断で「1件だけなら実投稿してよい」として許可）。結果、**旧ペルソナ（技術者ネタ）のメモがそのまま投稿された**（「技術面接の対策、AIに想定質問出してもらうだけじゃなくて〜」＋`#技術面接 #AI活用`）。原因は`fetch_latest_memo`が`used_at IS NULL`の最古1件を機械的に取得するため、転換前に溜まっていた旧メモが先に読まれたこと。プロンプト変更自体の不具合ではなく、素材となるメモが旧ペルソナのものだったために起きた想定内の結果
- **`data-collector`を実行してゲーム開発ネタを`memo_mq`に投入**（ローカル`agent-wrapper`をAGENT_SDK_URLに指定）。Qiitaの`tag:ゲーム開発`クエリが実際に機能し、脱出ゲーム制作・Axmol Engine・インベントリシステムなど大量のゲーム開発文脈メモを生成（36件中30件成功、1件はOpenAI API呼び出しの一時的なTLS接続エラー、5件はYouTube側で既に処理済みのためスキップ）。Agent SDKも正常に2件生成・保存を複数回確認。この初回実行で一括生成が起きた反省を踏まえ、Qiitaの`per_page`を3に制限（上記「決定事項」参照）
- **`memo_mq`のクリーンアップはユーザー側で対応済み**: 転換前（技術者ペルソナ時代）に溜まっていた未使用の古いメモは、ユーザーが別途対応した

### クロージング

- `HANDOFF_PHASE5.md`は完了に伴い削除済み
- **今後の残課題**（Phase5の範囲外、次回以降）
  - 新ペルソナでの実投稿による最終確認（`memo_mq`クリーンアップ後の`cargo run`）はまだ実施していない
  - `SNS_POSITIONING_PROPOSAL.md`の更新（Phase A該当部分が完了した旨の追記）はまだ行っていない
  - SNS担当者から提案のあったエンゲージメント施策（末尾に問いかけ・二択を添えてリプライを誘発する`<rules>`追加）は、ペルソナ転換とは別軸の改善として保留。段階的に分離する方針で合意済み
  - Railwayへのデプロイ・環境変数反映はまだ行っていない
  - data-collector側の`.env`に`AGENT_SDK_URL` / `AGENT_SDK_API_KEY`のローカル用設定がまだ無い。今回はコマンドラインで一時的に環境変数を渡して実行した
