//! 开发辅助：打印一段代码每个 token 的 scope 栈，用于校准主题选择器。
//! 用法：cargo run --example scopes

use syntect::highlighting::Highlighter;
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};

fn main() {
    let syntax_set = SyntaxSet::load_defaults_newlines();
    let syntax = syntax_set
        .find_syntax_by_extension("rs")
        .expect("rust syntax");

    let pack = coderender::theme::ThemePack::builtin().expect("builtin theme pack");
    let palette = pack.palette("mocha").expect("mocha palette");
    let theme = coderender::syntax::syntect_theme(&palette);
    let highlighter = Highlighter::new(&theme);

    let code = r#"use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Entry<V> {
    pub value: V,
    pub hits: u64,
}

impl<K: Clone, V: Clone> Entry<V> {
    pub fn get(&self, key: &K) -> Option<V> {
        let s = "text";
        let n = 42u64;
        // comment
        Some(value)
    }
}
"#;

    let mut parse_state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();

    for line in code.split_inclusive('\n') {
        let ops = parse_state.parse_line(line, &syntax_set).unwrap();
        let mut pos = 0usize;
        let report = |text: &str, stack: &ScopeStack| {
            if text.trim().is_empty() {
                return;
            }
            let style = highlighter.style_for_stack(&stack.scopes);
            println!(
                "{:<16} #{:02x}{:02x}{:02x}  {}",
                text.trim_end(),
                style.foreground.r,
                style.foreground.g,
                style.foreground.b,
                stack
                    .scopes
                    .iter()
                    .map(|scope| scope.to_string())
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        };

        for (offset, op) in &ops {
            if *offset > pos {
                report(&line[pos..*offset], &stack);
                pos = *offset;
            }
            stack.apply(op).unwrap();
        }
        if pos < line.len() {
            report(&line[pos..], &stack);
        }
    }
}
