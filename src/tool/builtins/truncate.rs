pub const DEFAULT_MAX_LINES: usize = 2_000;
pub const DEFAULT_MAX_BYTES: usize = 50 * 1_024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TruncatedBy {
    Lines,
    Bytes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Truncation {
    pub content: String,
    pub truncated: bool,
    pub truncated_by: Option<TruncatedBy>,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub output_lines: usize,
    pub output_bytes: usize,
    pub first_line_exceeds_limit: bool,
    pub first_line_partial: bool,
}

pub fn truncate_head(content: &str, max_lines: usize, max_bytes: usize) -> Truncation {
    let lines = lines_for_counting(content);
    let total_lines = lines.len();
    let total_bytes = content.len();
    if total_lines <= max_lines && total_bytes <= max_bytes {
        return complete(content, total_lines, total_bytes);
    }

    if lines.first().is_some_and(|line| line.len() > max_bytes) {
        return Truncation {
            content: String::new(),
            truncated: true,
            truncated_by: Some(TruncatedBy::Bytes),
            total_lines,
            total_bytes,
            output_lines: 0,
            output_bytes: 0,
            first_line_exceeds_limit: true,
            first_line_partial: false,
        };
    }

    let mut output = Vec::new();
    let mut bytes: usize = 0;
    let mut truncated_by = TruncatedBy::Lines;
    for line in lines.iter().take(max_lines) {
        let line_bytes = line.len() + usize::from(!output.is_empty());
        if bytes.saturating_add(line_bytes) > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            break;
        }
        output.push(*line);
        bytes += line_bytes;
    }
    if truncated_by != TruncatedBy::Bytes && output.len() >= max_lines {
        truncated_by = TruncatedBy::Lines;
    }
    let content = output.join("\n");
    Truncation {
        output_bytes: content.len(),
        output_lines: output.len(),
        content,
        truncated: true,
        truncated_by: Some(truncated_by),
        total_lines,
        total_bytes,
        first_line_exceeds_limit: false,
        first_line_partial: false,
    }
}

pub fn truncate_tail(content: &str, max_lines: usize, max_bytes: usize) -> Truncation {
    let lines = lines_for_counting(content);
    let total_lines = lines.len();
    let total_bytes = content.len();
    if total_lines <= max_lines && total_bytes <= max_bytes {
        return complete(content, total_lines, total_bytes);
    }

    let mut output = Vec::new();
    let mut bytes: usize = 0;
    let mut truncated_by = TruncatedBy::Lines;
    let mut first_line_partial = false;
    for line in lines.iter().rev().take(max_lines) {
        let line_bytes = line.len() + usize::from(!output.is_empty());
        if bytes.saturating_add(line_bytes) > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            if output.is_empty() {
                let partial = suffix_at_char_boundary(line, max_bytes);
                output.push(partial);
                first_line_partial = true;
            }
            break;
        }
        output.push((*line).to_owned());
        bytes += line_bytes;
    }
    output.reverse();
    if truncated_by != TruncatedBy::Bytes && output.len() >= max_lines {
        truncated_by = TruncatedBy::Lines;
    }
    let content = output.join("\n");
    Truncation {
        output_bytes: content.len(),
        output_lines: output.len(),
        content,
        truncated: true,
        truncated_by: Some(truncated_by),
        total_lines,
        total_bytes,
        first_line_exceeds_limit: false,
        first_line_partial,
    }
}

fn complete(content: &str, total_lines: usize, total_bytes: usize) -> Truncation {
    Truncation {
        content: content.to_owned(),
        truncated: false,
        truncated_by: None,
        total_lines,
        total_bytes,
        output_lines: total_lines,
        output_bytes: total_bytes,
        first_line_exceeds_limit: false,
        first_line_partial: false,
    }
}

fn lines_for_counting(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut lines = content.split('\n').collect::<Vec<_>>();
    if content.ends_with('\n') {
        lines.pop();
    }
    lines
}

fn suffix_at_char_boundary(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut start = text.len() - max_bytes;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_head_without_partial_lines() {
        let result = truncate_head("one\ntwo\nthree", 2, 100);
        assert_eq!(result.content, "one\ntwo");
        assert_eq!(result.truncated_by, Some(TruncatedBy::Lines));
    }

    #[test]
    fn truncates_tail_on_utf8_boundaries() {
        let result = truncate_tail("前缀-abcdef", 10, 5);
        assert_eq!(result.content, "bcdef");
        assert!(result.first_line_partial);
    }
}
