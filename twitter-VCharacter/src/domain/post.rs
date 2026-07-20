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
}
