pub fn mask_key(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "****".to_string();
    }
    let suffix: String = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let prefix = if trimmed.starts_with("sk-") {
        "sk-"
    } else {
        ""
    };
    format!("{prefix}****{suffix}")
}

pub fn redact_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let chars: Vec<char> = value.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        let remaining: String = chars[index..].iter().collect();
        if remaining.to_ascii_lowercase().starts_with("bearer ") {
            output.push_str("Bearer ****");
            index += "Bearer ".chars().count();
            while index < chars.len() && !chars[index].is_whitespace() {
                index += 1;
            }
            continue;
        }
        if remaining.starts_with("sk-") {
            let start = index;
            index += 3;
            while index < chars.len()
                && !chars[index].is_whitespace()
                && !['"', '\'', ',', ';', ')', ']'].contains(&chars[index])
            {
                index += 1;
            }
            let token: String = chars[start..index].iter().collect();
            output.push_str(&mask_key(&token));
            continue;
        }
        output.push(chars[index]);
        index += 1;
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_short_and_long_keys() {
        assert_eq!(mask_key("abcd"), "****abcd");
        assert_eq!(mask_key("sk-1234567890"), "sk-****7890");
    }

    #[test]
    fn redacts_bearer_and_key_text() {
        let source = "Authorization: Bearer secret-value, key=sk-1234567890";
        let redacted = redact_text(source);
        assert!(!redacted.contains("secret-value"));
        assert!(!redacted.contains("1234567890"));
        assert!(redacted.contains("****7890"));
    }
}
