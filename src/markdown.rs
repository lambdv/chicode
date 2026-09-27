//! Adapt CommonMark blocks to Slint's small native styled-text renderer.
use pulldown_cmark::{Event, Parser, Tag, TagEnd};

/// Escape literal text so streamed markdown cannot become native HTML styling.
fn escape(text: &str) -> String {
    let mut result = String::new();
    for c in text.chars() { if "\\`*_{}[]<>#+-!.|".contains(c) { result.push('\\'); } result.push(c); }
    result
}

/// Render supported inline formatting and readable headings, code, quotes, and tables.
pub fn render(text: &str) -> slint::StyledText {
    let mut output = String::new();
    let mut code = false;
    for event in Parser::new(text) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { .. } | Tag::Strong => output.push_str("**"),
                Tag::Emphasis => output.push('*'),
                Tag::Strikethrough => output.push_str("~~"),
                Tag::CodeBlock(_) => { code = true; output.push_str("\n\n"); },
                Tag::Item => output.push_str("• "),
                Tag::BlockQuote(_) => output.push_str("│ "),
                _ => {},
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) => output.push_str("**\n\n"),
                TagEnd::Strong => output.push_str("**"),
                TagEnd::Emphasis => output.push('*'),
                TagEnd::Strikethrough => output.push_str("~~"),
                TagEnd::CodeBlock => { code = false; output.push_str("\n\n"); },
                TagEnd::Paragraph | TagEnd::Item | TagEnd::BlockQuote(_) => output.push_str("\n\n"),
                _ => {},
            },
            Event::Text(value) if code => {
                for line in value.lines() {
                    let ticks = "`".repeat(line.split(|c| c != '`').map(str::len).max().unwrap_or(0) + 1);
                    output.push_str(&format!("{ticks} {line} {ticks}\n\n"));
                }
            },
            Event::Text(value) => output.push_str(&escape(&value)),
            Event::Code(value) => {
                let ticks = "`".repeat(value.split(|c| c != '`').map(str::len).max().unwrap_or(0) + 1);
                output.push_str(&format!("{ticks} {value} {ticks}"));
            },
            Event::SoftBreak | Event::HardBreak => output.push_str("  \n"),
            Event::Rule => output.push_str("\n\n────────\n\n"),
            Event::Html(value) | Event::InlineHtml(value) => output.push_str(&escape(&value)),
            _ => {},
        }
    }
    slint::StyledText::from_markdown(&output).unwrap_or_else(|_| slint::StyledText::from_plain_text(text))
}

#[cfg(test)]
mod tests {
    /// Common assistant output must remain formatted despite unsupported native block syntax.
    #[test]
    fn headings_and_code_render_without_raw_markdown_fallback() {
        let source = "# Result\n\n**Done**\n\n```rust\nlet x = \"<div>\";\n```\n\n> quoted";
        assert_ne!(super::render(source), slint::StyledText::from_plain_text(source));
    }
}
