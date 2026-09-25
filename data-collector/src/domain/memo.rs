//! memo_mq の入口ゲート。
//!
//! memo_mq に入るメモはすべて「ツイートの素材として成立している」ことを保証する。
//! YouTube / Qiita / Agent の3ソースすべてがここを通る唯一の入口なので、内容の判定はここを正とする
//! （agent-wrapper 側の `_is_valid_memo` は「どの行がメモか」を切り出すパースの責任で、一部の規則が重なる）。

const MIN_CHARS: usize = 15;
const MAX_CHARS: usize = 100;

/// 箇条書き・見出し記号
const HEADING_PREFIXES: [char; 6] = ['-', '*', '・', '#', '>', '•'];

/// メモではなく、AIの作業報告・前置き・応答であることを示す言い回し
const META_PHRASES: [&str; 30] = [
    "以下、", "以下,", "以下：", "以下:", "以下です", "以下の",
    "CLAUDE.md", "TASK_PROMPT", "WebSearch",
    "文字以内", "字以内", "収まって",
    "作成しました", "作成します", "深掘りしました", "確定します", "確定しました",
    "トピックが固ま", "トピックが決ま",
    "メモを確定", "メモを作成", "メモを出力", "メモをまとめ",
    "了解", "承知しました", "かしこまりました",
    "Sources", "参考:", "参考：", "出典",
];

#[derive(Debug, PartialEq, Eq)]
pub enum MemoRejection {
    TooShort,
    TooLong,
    ContainsUrl,
    Heading,
    Numbered,
    TrailingColon,
    Markdown,
    MetaPhrase(&'static str),
}

/// ソース共通の検査。YouTube / Qiita のメモは「メモ: 」接頭辞の有無どちらで渡してもよい
pub fn validate_memo(memo: &str) -> Result<(), MemoRejection> {
    let text = strip_memo_prefix(memo.trim());
    let len = text.chars().count();

    if len < MIN_CHARS {
        return Err(MemoRejection::TooShort);
    }
    if len > MAX_CHARS {
        return Err(MemoRejection::TooLong);
    }
    if text.contains("http://") || text.contains("https://") {
        return Err(MemoRejection::ContainsUrl);
    }
    if text.starts_with(HEADING_PREFIXES) || starts_with_memo_number(text) {
        return Err(MemoRejection::Heading);
    }
    if starts_with_list_number(text) {
        return Err(MemoRejection::Numbered);
    }
    if text.ends_with(':') || text.ends_with('：') {
        return Err(MemoRejection::TrailingColon);
    }
    if text.contains("**") {
        return Err(MemoRejection::Markdown);
    }
    if let Some(phrase) = META_PHRASES.iter().find(|p| text.contains(*p)) {
        return Err(MemoRejection::MetaPhrase(phrase));
    }
    Ok(())
}

fn strip_memo_prefix(text: &str) -> &str {
    text.strip_prefix("メモ:")
        .or_else(|| text.strip_prefix("メモ："))
        .map(str::trim_start)
        .unwrap_or(text)
}

/// 「メモ1」「メモ 2」のような見出し
fn starts_with_memo_number(text: &str) -> bool {
    text.strip_prefix("メモ")
        .map(|rest| rest.trim_start().starts_with(|c: char| c.is_ascii_digit()))
        .unwrap_or(false)
}

/// 「1. 」「2)」のような番号付け
fn starts_with_list_number(text: &str) -> bool {
    let rest = text.trim_start_matches(|c: char| c.is_ascii_digit());
    rest.len() < text.len() && rest.starts_with(['.', ')', '．', '）'])
}

#[cfg(test)]
mod tests {
    use super::*;

    // 実際に memo_mq に入った正常なメモ（source ごとに代表例）
    #[test]
    fn accepts_real_memos() {
        let memos = [
            "メモ: Axmol Engineのパーティクルシステムを使うと、火の表現が簡単に調整できるのが面白い。自分なりのパーティクルも試してみたくなる！",
            "1週間で1本完成させる縛り、肥大化を防ぐ発想が面白くて自分もやってみたい",
            "生成AIへの現場の評価、去年よりむしろ厳しくなってるらしくて意外だった",
            "個人開発、数ヶ月後の自分が一番の他人。メモ残す習慣は大事にしたい",
        ];
        for m in memos {
            assert_eq!(validate_memo(m), Ok(()), "{m}");
        }
    }

    // 2026-09-02 / 09-24 に実際に投稿されてしまったメモ
    #[test]
    fn rejects_memos_that_leaked_to_x() {
        assert_eq!(validate_memo("Sources:"), Err(MemoRejection::TooShort));
        assert_eq!(
            validate_memo("2つのトピックが固まったので、メモを確定します。"),
            Err(MemoRejection::MetaPhrase("確定します"))
        );
    }

    // 2026-08 の棚卸しで見つかった破損パターン（PROGRESS.md「2026-08 障害対応 ③」）
    #[test]
    fn rejects_known_broken_patterns() {
        let broken = [
            "以下、最終メモ2個です。これを使ってください",
            "**メモ1（Game A Week）** 1週間で作る",
            "両方とも50文字以内に収まっています。問題ありません",
            "CLAUDE.mdのルールに従って感想形式のメモを2個作成します。",
            "参考記事はこちら https://example.com/article を見てください",
            "- Game A Weekで1週間に1本作る練習法を見つけた",
            "1. インディーの資金調達について調べてみた結果",
            "メモ2: クラファンはマーケ施策だと気づいたので面白い",
            "了解です。メモを確定したのでお送りしますね",
        ];
        for b in broken {
            assert!(validate_memo(b).is_err(), "{b}");
        }
    }

    #[test]
    fn strips_prefix_before_checking_length() {
        // 接頭辞を除くと15字未満になるものは弾く
        assert_eq!(validate_memo("メモ: 面白い"), Err(MemoRejection::TooShort));
    }
}
