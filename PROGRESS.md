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

### 未決事項（次回詰める）

- Agentへの指示プロンプトの具体的な内容（「トレンドを調べて」だけでは曖昧すぎるため、対象範囲・除外基準等の具体化が必要。ユーザーと後日一緒に詰める）
- Python wrapperアプリの技術選定（FastAPI等のフレームワーク、Claude Agent SDKの具体的な組み込み方）
- `AgentPort`のRust側インターフェース設計（`investigate() -> Result<String>`のようなシンプルな形になる見込み）
- Python wrapperアプリ↔Rust間の認証・エラーハンドリング（HTTPタイムアウト、Agent側の例外の伝搬方法等）
