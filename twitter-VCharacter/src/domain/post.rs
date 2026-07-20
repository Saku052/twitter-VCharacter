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
}
