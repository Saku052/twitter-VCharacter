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
