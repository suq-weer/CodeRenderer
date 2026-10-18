//! 模糊匹配（"Did you mean"）。
//!
//! 当用户指定的字体、配色、语言、包 ID 等没有严格命中时，用它计算相近候选，
//! 让报错信息带上「您是否指的是……」。

use crate::theme::fold_key;

/// 距离上限：输入越短越严格，最长容忍 5 个编辑距离。
fn threshold(input: &str) -> u8 {
    let len = input.chars().count() as u8;
    (len / 3 + 2).clamp(2, 5)
}

/// 从候选里挑出与 `input` 相近的条目，按距离升序、名称升序排列。
pub fn suggestions(input: &str, candidates: &[String], limit: usize) -> Vec<String> {
    let wanted = fold_key(input);
    if wanted.is_empty() || candidates.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(u8, String)> = Vec::new();
    for candidate in candidates {
        let key = fold_key(candidate);
        if key.is_empty() {
            continue;
        }
        if let Some(distance) = score(&wanted, &key) {
            scored.push((distance, candidate.clone()));
        }
    }

    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    scored.dedup_by(|a, b| a.1 == b.1);
    scored.into_iter().take(limit).map(|(_, name)| name).collect()
}

/// 候选与输入的匹配得分；越小越相近，`None` 表示不相关。
fn score(wanted: &str, key: &str) -> Option<u8> {
    if key == wanted {
        return None; // 归一化后完全相同：调用方本该已经命中
    }
    let prefix = common_prefix_len(wanted, key);
    // 前缀基本一致（例如只打了前半截）时视为最相近
    if prefix >= 4 && prefix * 10 >= wanted.chars().count() * 6 {
        return Some(1);
    }
    // 互相包含通常就是拼写不完整/多打了字
    if key.contains(wanted) || wanted.contains(key) {
        return Some(1);
    }
    let distance = levenshtein(wanted, key);
    (distance <= threshold(wanted)).then_some(distance)
}

/// 两个字符串（已归一化）的公共前缀长度。
fn common_prefix_len(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

/// 生成「，您是否指的是：A / B」这样的后缀（可直接接在句子后面）；
/// 没有候选时返回空串。
pub fn did_you_mean(input: &str, candidates: &[String], limit: usize) -> String {
    let hits = suggestions(input, candidates, limit);
    if hits.is_empty() {
        String::new()
    } else {
        format!("，您是否指的是：{}", hits.join(" / "))
    }
}

/// Levenshtein 距离（按字符，最多到 255 就封顶）。
fn levenshtein(a: &str, b: &str) -> u8 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len().min(255) as u8;
    }
    if b.is_empty() {
        return a.len().min(255) as u8;
    }

    let mut previous: Vec<u8> = (0..=b.len()).map(|i| i.min(255) as u8).collect();
    let mut current = vec![0u8; b.len() + 1];

    for (i, ca) in a.iter().enumerate() {
        current[0] = (i + 1).min(255) as u8;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            let deletion = previous[j + 1] + 1;
            let insertion = current[j] + 1;
            let substitution = previous[j] + cost;
            current[j + 1] = deletion.min(insertion).min(substitution);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn finds_close_typos() {
        let candidates = owned(&["latte", "frappe", "macchiato", "mocha"]);
        assert_eq!(suggestions("lattee", &candidates, 3), vec!["latte"]);
        // frappé 归一化后就是 frappe：调用方本该直接命中，不该被当成“您是否指的是”
        assert!(!suggestions("frappé", &candidates, 3).contains(&"frappe".to_string()));
        let hits = suggestions("macchaito", &candidates, 3);
        assert_eq!(hits.first().unwrap(), "macchiato", "{hits:?}");
        assert!(hits.contains(&"macchiato".to_string()));
    }

    #[test]
    fn prefix_typos_rank_first() {
        let candidates = owned(&[
            "JetBrainsMono Nerd Font Mono",
            "JetBrainsMono NFM",
            "Consolas",
        ]);
        let hits = suggestions("JetBrains Monoo", &candidates, 3);
        assert_eq!(
            hits.first().unwrap(),
            "JetBrainsMono Nerd Font Mono",
            "{hits:?}"
        );
    }

    #[test]
    fn prefix_and_substring_rank_first() {
        let candidates = owned(&["JetBrainsMono Nerd Font Mono", "JetBrains Mono", "Consolas"]);
        let hits = suggestions("jetbrainsmono", &candidates, 3);
        assert_eq!(hits.first().unwrap(), "JetBrainsMono Nerd Font Mono");
    }

    #[test]
    fn unrelated_input_has_no_suggestion() {
        let candidates = owned(&["latte", "mocha"]);
        assert!(suggestions("zzzzzzzzzz", &candidates, 3).is_empty());
        assert!(did_you_mean("zzzzzzzzzz", &candidates, 3).is_empty());
    }

    #[test]
    fn message_format() {
        let candidates = owned(&["mocha", "latte"]);
        let message = did_you_mean("mocah", &candidates, 3);
        assert_eq!(message, "，您是否指的是：mocha");
        assert!(did_you_mean("zzzzzz", &candidates, 3).is_empty());
    }

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("flaw", "lawn"), 2);
    }
}
