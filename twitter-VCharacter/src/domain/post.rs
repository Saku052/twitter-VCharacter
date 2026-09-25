pub fn parse_image_post_probability(raw: Option<String>) -> f64 {
    const DEFAULT_PROBABILITY: f64 = 0.3;
    raw.and_then(|v| v.parse::<f64>().ok())
        .map(|v| v.clamp(0.0, 1.0))
        .unwrap_or(DEFAULT_PROBABILITY)
}

pub fn should_attach_image(random_value: f64, probability: f64) -> bool {
    random_value < probability
}

pub fn parse_tags(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|tag| tag.trim().trim_start_matches('#').trim())
        .filter(|tag| !tag.is_empty())
        .map(String::from)
        .collect()
}

// TODO: 将来はMySQLの履歴・PostgreSQLの感情データを元に編集する
pub fn prepare_post(body: String, tags: Vec<String>) -> String {
    let tags_str = tags
        .iter()
        .map(|tag| format!("#{}", tag))
        .collect::<Vec<_>>()
        .join(" ");
    format!("{}\n\n{}", body, tags_str)
}

/// 本文生成プロンプト（BODY_SYS_PRPT）で「素材として成立しないメモ」に対して出力させる合図
pub const SKIP_SENTINEL: &str = "SKIP";
const MAX_BODY_CHARS: usize = 140;

/// 生成AIがツイートではなく「アシスタントとしての応答」を返したことを示す言い回し。
/// 2026-09-02 / 09-24 に実際に投稿された2件はどちらもここで止まる
const ASSISTANT_REPLY_PHRASES: [&str; 16] = [
    "了解です", "了解しました", "承知しました", "かしこまりました",
    "送ってください", "お送りください", "メモを送", "メモを貼", "メモを共有", "メモを教えて",
    "ツイート化したい", "本文に整え", "本人視点", "140字以内の本文",
    "メモがありません", "メモが見当たりません",
];

#[derive(Debug, PartialEq, Eq)]
pub enum BodyRejection {
    /// 生成AI自身が「素材にならない」と判断した（SKIP 契約）
    ModelSkip,
    Empty,
    TooLong,
    ContainsUrl,
    AssistantReply(&'static str),
}

impl BodyRejection {
    /// memo_mq.skipped_reason に保存するコード
    pub fn code(&self) -> String {
        match self {
            BodyRejection::ModelSkip => "model_skip".to_string(),
            BodyRejection::Empty => "empty".to_string(),
            BodyRejection::TooLong => "too_long".to_string(),
            BodyRejection::ContainsUrl => "url".to_string(),
            BodyRejection::AssistantReply(p) => format!("assistant_reply:{}", p),
        }
    }
}

/// 投稿直前の出力ガード。ここで弾いた本文は絶対に投稿しない
pub fn validate_body(body: &str) -> Result<(), BodyRejection> {
    let body = body.trim();
    if body.starts_with(SKIP_SENTINEL) {
        return Err(BodyRejection::ModelSkip);
    }
    if body.is_empty() {
        return Err(BodyRejection::Empty);
    }
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(BodyRejection::TooLong);
    }
    if body.contains("http://") || body.contains("https://") {
        return Err(BodyRejection::ContainsUrl);
    }
    if let Some(phrase) = ASSISTANT_REPLY_PHRASES.iter().find(|p| body.contains(*p)) {
        return Err(BodyRejection::AssistantReply(phrase));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combines_body_and_tags_with_blank_line() {
        let body = "Rustの所有権、やっと腑に落ちてきた".to_string();
        let tags = vec!["Rust".to_string(), "個人開発".to_string()];

        let result = prepare_post(body, tags);

        assert_eq!(result, "Rustの所有権、やっと腑に落ちてきた\n\n#Rust #個人開発");
    }

    #[test]
    fn handles_empty_tags() {
        let body = "今日は特にタグなし".to_string();
        let tags = vec![];

        let result = prepare_post(body, tags);

        assert_eq!(result, "今日は特にタグなし\n\n");
    }

    #[test]
    fn parse_tags_splits_by_comma_and_trims() {
        let result = parse_tags("Rust, 個人開発");

        assert_eq!(result, vec!["Rust".to_string(), "個人開発".to_string()]);
    }

    #[test]
    fn parse_tags_removes_empty_elements_from_consecutive_commas() {
        let result = parse_tags("Rust,,個人開発");

        assert_eq!(result, vec!["Rust".to_string(), "個人開発".to_string()]);
    }

    #[test]
    fn parse_tags_removes_empty_elements_from_trailing_comma() {
        let result = parse_tags("Rust,個人開発,");

        assert_eq!(result, vec!["Rust".to_string(), "個人開発".to_string()]);
    }

    #[test]
    fn parse_tags_strips_leading_hash_without_double_hash() {
        let result = parse_tags("#Rust,個人開発");

        assert_eq!(result, vec!["Rust".to_string(), "個人開発".to_string()]);
    }

    #[test]
    fn parse_image_post_probability_uses_valid_value() {
        assert_eq!(parse_image_post_probability(Some("0.5".to_string())), 0.5);
    }

    #[test]
    fn parse_image_post_probability_falls_back_to_default_when_unset() {
        assert_eq!(parse_image_post_probability(None), 0.3);
    }

    #[test]
    fn parse_image_post_probability_falls_back_to_default_when_unparseable() {
        assert_eq!(parse_image_post_probability(Some("not-a-number".to_string())), 0.3);
    }

    #[test]
    fn parse_image_post_probability_clamps_values_above_one() {
        assert_eq!(parse_image_post_probability(Some("5.0".to_string())), 1.0);
    }

    #[test]
    fn parse_image_post_probability_clamps_negative_values() {
        assert_eq!(parse_image_post_probability(Some("-1.0".to_string())), 0.0);
    }

    #[test]
    fn should_attach_image_at_zero_probability_never_attaches() {
        assert!(!should_attach_image(0.0, 0.0));
        assert!(!should_attach_image(0.999999, 0.0));
    }

    #[test]
    fn should_attach_image_at_full_probability_always_attaches() {
        assert!(should_attach_image(0.0, 1.0));
        assert!(should_attach_image(0.999999, 1.0));
    }

    #[test]
    fn should_attach_image_at_mid_probability_respects_threshold() {
        assert!(should_attach_image(0.2, 0.5));
        assert!(!should_attach_image(0.5, 0.5));
        assert!(!should_attach_image(0.8, 0.5));
    }

    // 2026-09-02 / 09-24 に実際に投稿されてしまった本文
    #[test]
    fn validate_body_rejects_assistant_replies_that_leaked_to_x() {
        let leaked = [
            "了解です！\n確定したメモを送ってください。そこから本人視点のツイート本文に整えます。",
            "ツイート化したいメモを送ってください！  \n内容を元に、本人視点で140字以内の本文にします。",
        ];
        for body in leaked {
            assert!(matches!(validate_body(body), Err(BodyRejection::AssistantReply(_))), "{body}");
        }
    }

    #[test]
    fn validate_body_accepts_real_posted_bodies() {
        let bodies = [
            "1週間で1本ゲームを完成させる縛り、かなりキツそうだけどスコープ削る訓練になりそうで試したい。完成させる筋肉ほしい🎮",
            "個人開発、数ヶ月後の自分が一番の他人なんだよな。久々にコード読むと「誰が書いたんこれ」ってなるので、未来の自分向けにメモ残す習慣ちゃんと大事にしたい…📝",
            "生成AI、悪影響だと思う人が52%いる一方で、実際に使ってる人も36%いるのめちゃ現場感ある。便利さと怖さ、どっちも本音なんだろうな🤔",
        ];
        for body in bodies {
            assert_eq!(validate_body(body), Ok(()), "{body}");
        }
    }

    #[test]
    fn validate_body_detects_skip_sentinel_with_surrounding_whitespace() {
        assert_eq!(validate_body("SKIP"), Err(BodyRejection::ModelSkip));
        assert_eq!(validate_body("  SKIP\n"), Err(BodyRejection::ModelSkip));
    }

    #[test]
    fn validate_body_rejects_empty_long_and_url() {
        assert_eq!(validate_body("   "), Err(BodyRejection::Empty));
        assert_eq!(validate_body(&"あ".repeat(141)), Err(BodyRejection::TooLong));
        assert_eq!(validate_body(&"あ".repeat(140)), Ok(()));
        assert_eq!(validate_body("詳しくは https://example.com を見てほしい"), Err(BodyRejection::ContainsUrl));
    }

    #[test]
    fn rejection_code_is_stable_for_db() {
        assert_eq!(BodyRejection::ModelSkip.code(), "model_skip");
        assert_eq!(BodyRejection::AssistantReply("了解です").code(), "assistant_reply:了解です");
    }
}
