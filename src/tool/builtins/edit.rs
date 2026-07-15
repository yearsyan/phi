use std::{
    borrow::Cow,
    fmt::Write as _,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use super::common::{io_error, mutation_guard, normalize_cwd, resolve_path};
use crate::{
    error::ToolError,
    tool::{Tool, ToolEffect, ToolOutput},
    types::ToolDefinition,
};

#[derive(Clone, Debug)]
pub struct EditTool {
    cwd: PathBuf,
    max_file_size: usize,
}

/// Default upper bound for a file edited with [`EditTool`].
pub const DEFAULT_MAX_EDIT_BYTES: usize = 16 * 1_024 * 1_024;

const DIFF_CONTEXT_LINES: usize = 1;
const MAX_DIFF_HUNKS: usize = 6;
const MAX_DIFF_LINES: usize = 120;
const MAX_DIFF_BYTES: usize = 12 * 1_024;

impl EditTool {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: normalize_cwd(cwd),
            max_file_size: DEFAULT_MAX_EDIT_BYTES,
        }
    }

    /// Sets the maximum source file size accepted by this tool.
    pub fn max_file_size(mut self, max_bytes: usize) -> Self {
        self.max_file_size = max_bytes.max(1);
        self
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Edit {
    old_text: String,
    new_text: String,
    #[serde(default)]
    replace_all: bool,
}

struct EditArguments {
    path: String,
    edits: Vec<Edit>,
}

impl EditArguments {
    fn parse(mut value: Value) -> Result<Self, ToolError> {
        let object = value
            .as_object_mut()
            .ok_or_else(|| ToolError::new("invalid edit arguments: expected an object"))?;
        let path = object
            .remove("path")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| ToolError::new("invalid edit arguments: path must be a string"))?;

        let old_text = object
            .remove("oldText")
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    ToolError::new("invalid edit arguments: oldText must be a string")
                })
            })
            .transpose()?;
        let new_text = object
            .remove("newText")
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    ToolError::new("invalid edit arguments: newText must be a string")
                })
            })
            .transpose()?;
        let replace_all = object
            .remove("replaceAll")
            .map(|value| {
                value.as_bool().ok_or_else(|| {
                    ToolError::new("invalid edit arguments: replaceAll must be a boolean")
                })
            })
            .transpose()?;
        let mut edits = match object.remove("edits") {
            Some(Value::String(encoded)) => serde_json::from_str(&encoded).map_err(|error| {
                ToolError::new(format!(
                    "invalid edit arguments: edits is not valid JSON: {error}"
                ))
            })?,
            Some(value) => serde_json::from_value(value)
                .map_err(|error| ToolError::new(format!("invalid edit arguments: {error}")))?,
            None => Vec::new(),
        };
        match (old_text, new_text, replace_all) {
            (Some(old_text), Some(new_text), replace_all) => edits.push(Edit {
                old_text,
                new_text,
                replace_all: replace_all.unwrap_or(false),
            }),
            (None, None, None) => {}
            (None, None, Some(_)) => {
                return Err(ToolError::new(
                    "invalid edit arguments: replaceAll requires oldText and newText",
                ));
            }
            _ => {
                return Err(ToolError::new(
                    "invalid edit arguments: oldText and newText must be provided together",
                ));
            }
        }
        if !object.is_empty() {
            return Err(ToolError::new(format!(
                "invalid edit arguments: unknown field(s): {}",
                object.keys().cloned().collect::<Vec<_>>().join(", ")
            )));
        }
        if edits.is_empty() {
            return Err(ToolError::new(
                "invalid edit arguments: edits must contain at least one replacement",
            ));
        }
        Ok(Self { path, edits })
    }
}

#[derive(Clone, Debug)]
struct MatchedEdit {
    edit_index: usize,
    start: usize,
    length: usize,
    replacement: Arc<str>,
}

#[derive(Clone, Copy, Debug)]
struct AppliedEdit {
    old_start: usize,
    old_end: usize,
    new_start: usize,
    new_end: usize,
}

struct NormalizedText<'a> {
    text: Cow<'a, str>,
    // (normalized offset after a collapsed CRLF, cumulative bytes removed).
    // Lone CR bytes are replaced one-for-one and need no offset entry.
    offset_adjustments: Vec<(usize, usize)>,
}

impl<'a> NormalizedText<'a> {
    fn new(original: &'a str) -> Self {
        if !original.as_bytes().contains(&b'\r') {
            return Self {
                text: Cow::Borrowed(original),
                offset_adjustments: Vec::new(),
            };
        }

        let mut text = String::with_capacity(original.len());
        let mut offset_adjustments = Vec::new();
        let mut original_offset = 0usize;
        let mut removed = 0usize;
        while let Some(relative) = original[original_offset..].find('\r') {
            let cr = original_offset + relative;
            text.push_str(&original[original_offset..cr]);
            if original.as_bytes().get(cr + 1) == Some(&b'\n') {
                text.push('\n');
                original_offset = cr + 2;
                removed += 1;
                offset_adjustments.push((text.len(), removed));
            } else {
                text.push('\n');
                original_offset = cr + 1;
            }
        }
        text.push_str(&original[original_offset..]);
        Self {
            text: Cow::Owned(text),
            offset_adjustments,
        }
    }

    fn original_offset(&self, normalized_offset: usize) -> usize {
        let adjustment_count = self
            .offset_adjustments
            .partition_point(|(offset, _)| *offset <= normalized_offset);
        let removed = adjustment_count
            .checked_sub(1)
            .map_or(0, |index| self.offset_adjustments[index].1);
        normalized_offset + removed
    }
}

#[async_trait]
impl Tool for EditTool {
    fn effect(&self) -> ToolEffect {
        ToolEffect::WorkspaceWrite
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "edit",
            format!(
                "Edit one UTF-8 regular file up to {} bytes. Replacements are matched against one original-file snapshot and may not overlap. Each edits[].oldText must be unique unless replaceAll is true; an exact miss may use a unique straight/curly-quote equivalent.",
                self.max_file_size
            ),
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to edit (relative to the configured working directory or absolute)"
                    },
                    "edits": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "oldText": {
                                    "type": "string",
                                    "description": "Exact, unique text from the original file"
                                },
                                "newText": {
                                    "type": "string",
                                    "description": "Replacement text"
                                },
                                "replaceAll": {
                                    "type": "boolean",
                                    "default": false,
                                    "description": "Replace every exact, non-overlapping occurrence in the original file"
                                }
                            },
                            "required": ["oldText", "newText"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["path", "edits"],
                "additionalProperties": false
            }),
        )
    }

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, ToolError> {
        let arguments = EditArguments::parse(arguments)?;
        let path = resolve_path(&self.cwd, &arguments.path)?;
        let _guard = mutation_guard(&path).await;
        let raw_content = match read_edit_file(&path, self.max_file_size).await {
            Ok(content) => content,
            Err(error) => return Err(edit_read_error(&path, error).await),
        };
        let (bom, content) = raw_content
            .strip_prefix('\u{feff}')
            .map_or(("", raw_content.as_str()), |content| ("\u{feff}", content));
        let normalized = NormalizedText::new(content);
        let mut matches = match_edits(normalized.text.as_ref(), &arguments.edits, &arguments.path)?;
        matches.sort_by_key(|edit| edit.start);

        for pair in matches.windows(2) {
            let previous = &pair[0];
            let current = &pair[1];
            if previous.start + previous.length > current.start {
                return Err(ToolError::new(format!(
                    "edits[{}] and edits[{}] overlap in {}",
                    previous.edit_index, current.edit_index, arguments.path
                )));
            }
        }

        let replacement_count = matches.len();
        let mut applied = Vec::with_capacity(matches.len());
        let mut edited = String::with_capacity(content.len());
        let mut raw_cursor = 0usize;
        for edit in &matches {
            let raw_start = normalized.original_offset(edit.start);
            let raw_end = normalized.original_offset(edit.start + edit.length);
            let replacement =
                preserve_local_line_endings(&edit.replacement, content, raw_start, raw_end);
            edited.push_str(&content[raw_cursor..raw_start]);
            let new_start = edited.len();
            edited.push_str(&replacement);
            let new_end = edited.len();
            applied.push(AppliedEdit {
                old_start: raw_start,
                old_end: raw_end,
                new_start,
                new_end,
            });
            raw_cursor = raw_end;
        }
        edited.push_str(&content[raw_cursor..]);
        if edited == content {
            return Err(ToolError::new(format!(
                "no changes made to {}: replacements produced identical content",
                arguments.path
            )));
        }

        let diff = compact_unified_diff(&arguments.path, content, &edited, &applied);
        let final_content = format!("{bom}{edited}");
        tokio::fs::write(&path, final_content.as_bytes())
            .await
            .map_err(|error| io_error("could not edit", &path, error))?;
        let summary = format!(
            "Successfully applied {} edit block(s), replacing {replacement_count} occurrence(s) in {}.\n\n{diff}",
            arguments.edits.len(),
            arguments.path
        );
        Ok(ToolOutput::success(summary).with_metadata(json!({
            "kind": "file_edit",
            "path": arguments.path,
            "editBlocks": arguments.edits.len(),
            "replacementCount": replacement_count,
            "originalBytes": content.len(),
            "editedBytes": edited.len(),
            "diff": diff,
        })))
    }
}

enum EditReadError {
    Io(io::Error),
    NotRegular,
    TooLarge { actual: u64, max: usize },
    InvalidUtf8(std::string::FromUtf8Error),
}

async fn read_edit_file(path: &Path, max_bytes: usize) -> Result<String, EditReadError> {
    let metadata = tokio::fs::metadata(path).await.map_err(EditReadError::Io)?;
    validate_edit_metadata(&metadata, max_bytes)?;

    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let file = options.open(path).await.map_err(EditReadError::Io)?;
    let opened_metadata = file.metadata().await.map_err(EditReadError::Io)?;
    validate_edit_metadata(&opened_metadata, max_bytes)?;

    let read_limit = max_bytes.saturating_add(1).try_into().unwrap_or(u64::MAX);
    let initial_capacity = usize::try_from(opened_metadata.len())
        .unwrap_or(max_bytes)
        .min(max_bytes);
    let mut bytes = Vec::with_capacity(initial_capacity);
    file.take(read_limit)
        .read_to_end(&mut bytes)
        .await
        .map_err(EditReadError::Io)?;
    if bytes.len() > max_bytes {
        return Err(EditReadError::TooLarge {
            actual: bytes.len() as u64,
            max: max_bytes,
        });
    }
    String::from_utf8(bytes).map_err(EditReadError::InvalidUtf8)
}

fn validate_edit_metadata(
    metadata: &std::fs::Metadata,
    max_bytes: usize,
) -> Result<(), EditReadError> {
    if !metadata.is_file() {
        return Err(EditReadError::NotRegular);
    }
    if metadata.len() > max_bytes as u64 {
        return Err(EditReadError::TooLarge {
            actual: metadata.len(),
            max: max_bytes,
        });
    }
    Ok(())
}

async fn edit_read_error(path: &Path, error: EditReadError) -> ToolError {
    match error {
        EditReadError::Io(error) if error.kind() == io::ErrorKind::NotFound => {
            let suggestion = find_similar_path(path)
                .await
                .map(|suggested| format!(" Did you mean {}?", suggested.display()))
                .unwrap_or_default();
            ToolError::new(format!(
                "could not edit {}: {error}.{suggestion}",
                path.display()
            ))
        }
        EditReadError::Io(error) => io_error("could not edit", path, error),
        EditReadError::NotRegular => ToolError::new(format!(
            "could not edit {}: path is not a regular file",
            path.display()
        )),
        EditReadError::TooLarge { actual, max } => ToolError::new(format!(
            "could not edit {}: file is {actual} bytes, exceeding the {max}-byte limit",
            path.display()
        )),
        EditReadError::InvalidUtf8(error) => ToolError::new(format!(
            "could not edit {}: file is not valid UTF-8 ({error})",
            path.display()
        )),
    }
}

async fn find_similar_path(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let target = path.file_name()?.to_str()?;
    let target_lower = target.to_lowercase();
    let target_stem = Path::new(target)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_lowercase);
    let mut entries = tokio::fs::read_dir(parent).await.ok()?;
    let mut best: Option<(usize, String, PathBuf)> = None;
    let mut inspected = 0usize;

    while inspected < 1_024 {
        let Some(entry) = entries.next_entry().await.ok()? else {
            break;
        };
        inspected += 1;
        if !entry
            .metadata()
            .await
            .is_ok_and(|metadata| metadata.is_file())
        {
            continue;
        }
        let candidate = entry.file_name();
        let Some(candidate) = candidate.to_str() else {
            continue;
        };
        let candidate_lower = candidate.to_lowercase();
        let candidate_stem = Path::new(candidate)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_lowercase);
        let score = if target_lower == candidate_lower {
            0
        } else if target_stem.is_some() && target_stem == candidate_stem {
            1
        } else {
            let maximum = target_lower.chars().count().clamp(2, 4);
            let Some(distance) = bounded_levenshtein(&target_lower, &candidate_lower, maximum)
            else {
                continue;
            };
            2 + distance
        };
        let key = (score, candidate_lower, entry.path());
        if best.as_ref().is_none_or(|current| key < *current) {
            best = Some(key);
        }
    }
    best.map(|(_, _, path)| path)
}

fn bounded_levenshtein(left: &str, right: &str, maximum: usize) -> Option<usize> {
    let left = left.chars().collect::<Vec<_>>();
    let right = right.chars().collect::<Vec<_>>();
    if left.len().abs_diff(right.len()) > maximum {
        return None;
    }
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    let mut current = vec![0usize; right.len() + 1];
    for (left_index, left_char) in left.iter().enumerate() {
        current[0] = left_index + 1;
        let mut row_minimum = current[0];
        for (right_index, right_char) in right.iter().enumerate() {
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(previous[right_index] + usize::from(left_char != right_char));
            row_minimum = row_minimum.min(current[right_index + 1]);
        }
        if row_minimum > maximum {
            return None;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    (previous[right.len()] <= maximum).then_some(previous[right.len()])
}

fn match_edits(content: &str, edits: &[Edit], path: &str) -> Result<Vec<MatchedEdit>, ToolError> {
    let mut matched = Vec::new();
    for (index, edit) in edits.iter().enumerate() {
        let old_text = normalize_to_lf(&edit.old_text);
        if old_text.is_empty() {
            return Err(ToolError::new(format!(
                "edits[{index}].oldText must not be empty in {path}"
            )));
        }
        let normalized_replacement = normalize_to_lf(&edit.new_text);
        let replacement: Arc<str> = Arc::from(normalized_replacement.as_ref());
        let mut occurrences = content.match_indices(old_text.as_ref());

        if edit.replace_all {
            let before = matched.len();
            matched.extend(occurrences.by_ref().map(|(start, _)| MatchedEdit {
                edit_index: index,
                start,
                length: old_text.len(),
                replacement: Arc::clone(&replacement),
            }));
            if matched.len() == before {
                return Err(not_found_error(index, path));
            }
            continue;
        }

        let Some((start, _)) = occurrences.next() else {
            match unique_curly_quote_match(content, &old_text) {
                QuoteMatch::Unique { start, length } => {
                    matched.push(MatchedEdit {
                        edit_index: index,
                        start,
                        length,
                        replacement,
                    });
                    continue;
                }
                QuoteMatch::Multiple => {
                    return Err(ToolError::new(format!(
                        "found multiple straight/curly-quote equivalents of edits[{index}] in {path}; oldText must be exact or uniquely identify one occurrence"
                    )));
                }
                QuoteMatch::None => return Err(not_found_error(index, path)),
            }
        };

        if occurrences.next().is_some() {
            let count = 2usize.saturating_add(occurrences.count());
            return Err(ToolError::new(format!(
                "found {count} occurrences of edits[{index}] in {path}; oldText must be unique or replaceAll must be true"
            )));
        }
        matched.push(MatchedEdit {
            edit_index: index,
            start,
            length: old_text.len(),
            replacement,
        });
    }
    Ok(matched)
}

fn not_found_error(index: usize, path: &str) -> ToolError {
    ToolError::new(format!(
        "could not find edits[{index}] in {path}; oldText must match exactly including whitespace and newlines"
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuoteMatch {
    None,
    Unique { start: usize, length: usize },
    Multiple,
}

/// Exact matching remains the primary contract. This fallback only accepts a
/// single match that differs solely by straight versus curly quote characters.
fn unique_curly_quote_match(content: &str, needle: &str) -> QuoteMatch {
    if needle.len() > 64 * 1_024 || !needle.chars().any(|ch| matches!(ch, '\'' | '"')) {
        return QuoteMatch::None;
    }
    let pattern = needle.chars().map(normalize_quote).collect::<Vec<_>>();
    let Some(&first) = pattern.first() else {
        return QuoteMatch::None;
    };
    let mut found = None;

    for (start, candidate_first) in content.char_indices() {
        if normalize_quote(candidate_first) != first {
            continue;
        }
        let mut candidate = content[start..].char_indices();
        let mut end = start;
        let mut changed_quote = false;
        let mut matched = true;
        for expected in &pattern {
            let Some((relative, actual)) = candidate.next() else {
                matched = false;
                break;
            };
            if normalize_quote(actual) != *expected {
                matched = false;
                break;
            }
            changed_quote |= actual != *expected && is_quote(actual);
            end = start + relative + actual.len_utf8();
        }
        if !matched || !changed_quote {
            continue;
        }
        if found.is_some() {
            return QuoteMatch::Multiple;
        }
        found = Some((start, end - start));
    }

    found.map_or(QuoteMatch::None, |(start, length)| QuoteMatch::Unique {
        start,
        length,
    })
}

fn normalize_quote(ch: char) -> char {
    match ch {
        '\u{2018}' | '\u{2019}' => '\'',
        '\u{201c}' | '\u{201d}' => '"',
        _ => ch,
    }
}

fn is_quote(ch: char) -> bool {
    matches!(
        ch,
        '\'' | '"' | '\u{2018}' | '\u{2019}' | '\u{201c}' | '\u{201d}'
    )
}

#[derive(Clone, Copy, Debug)]
struct DiffWindow {
    old_start: usize,
    old_end: usize,
    new_start: usize,
    new_end: usize,
}

fn compact_unified_diff(path: &str, original: &str, edited: &str, edits: &[AppliedEdit]) -> String {
    let mut windows: Vec<DiffWindow> = Vec::new();
    let mut truncated = false;
    for edit in edits {
        let (old_start, old_end) =
            line_window_bounds(original, edit.old_start, edit.old_end, DIFF_CONTEXT_LINES);
        let (new_start, new_end) =
            line_window_bounds(edited, edit.new_start, edit.new_end, DIFF_CONTEXT_LINES);
        if let Some(previous) = windows.last_mut()
            && old_start <= previous.old_end
            && new_start <= previous.new_end
        {
            previous.old_end = previous.old_end.max(old_end);
            previous.new_end = previous.new_end.max(new_end);
        } else if windows.len() < MAX_DIFF_HUNKS {
            windows.push(DiffWindow {
                old_start,
                old_end,
                new_start,
                new_end,
            });
        } else {
            truncated = true;
            break;
        }
    }

    let escaped_path = path.escape_default().to_string();
    let mut output = format!("--- a/{escaped_path}\n+++ b/{escaped_path}\n");
    let mut output_lines = 2usize;
    for window in windows {
        if output_lines >= MAX_DIFF_LINES || output.len() >= MAX_DIFF_BYTES {
            truncated = true;
            break;
        }
        let old_start_line = line_number_at(original, window.old_start);
        let new_start_line = line_number_at(edited, window.new_start);
        let old_window = normalize_to_lf(&original[window.old_start..window.old_end]);
        let new_window = normalize_to_lf(&edited[window.new_start..window.new_end]);
        let old_lines = lines_for_diff(&old_window);
        let new_lines = lines_for_diff(&new_window);
        let old_count = old_lines.len();
        let new_count = new_lines.len();
        let _ = writeln!(
            output,
            "@@ -{old_start_line},{old_count} +{new_start_line},{new_count} @@"
        );
        output_lines += 1;

        let common_prefix = old_lines
            .iter()
            .zip(&new_lines)
            .take_while(|(old, new)| old == new)
            .count();
        let maximum_suffix = old_lines
            .len()
            .saturating_sub(common_prefix)
            .min(new_lines.len().saturating_sub(common_prefix));
        let common_suffix = old_lines
            .iter()
            .rev()
            .zip(new_lines.iter().rev())
            .take(maximum_suffix)
            .take_while(|(old, new)| old == new)
            .count();

        for line in &old_lines[..common_prefix] {
            if !push_diff_line(&mut output, &mut output_lines, ' ', line) {
                truncated = true;
                break;
            }
        }
        if truncated {
            break;
        }
        for line in &old_lines[common_prefix..old_lines.len() - common_suffix] {
            if !push_diff_line(&mut output, &mut output_lines, '-', line) {
                truncated = true;
                break;
            }
        }
        if truncated {
            break;
        }
        for line in &new_lines[common_prefix..new_lines.len() - common_suffix] {
            if !push_diff_line(&mut output, &mut output_lines, '+', line) {
                truncated = true;
                break;
            }
        }
        if truncated {
            break;
        }
        for line in &old_lines[old_lines.len() - common_suffix..] {
            if !push_diff_line(&mut output, &mut output_lines, ' ', line) {
                truncated = true;
                break;
            }
        }
        if truncated {
            break;
        }
    }
    if truncated {
        let marker = "... [diff truncated; file changes were fully applied]\n";
        if output.len().saturating_add(marker.len()) <= MAX_DIFF_BYTES {
            output.push_str(marker);
        }
    }
    output
}

fn lines_for_diff(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines = text.split('\n').collect::<Vec<_>>();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

fn push_diff_line(output: &mut String, line_count: &mut usize, prefix: char, line: &str) -> bool {
    if *line_count >= MAX_DIFF_LINES || output.len() >= MAX_DIFF_BYTES {
        return false;
    }
    let available = MAX_DIFF_BYTES.saturating_sub(output.len() + prefix.len_utf8() + 1);
    if available == 0 {
        return false;
    }
    output.push(prefix);
    if line.len() <= available {
        output.push_str(line);
    } else {
        let marker = "... [line truncated]";
        let text_budget = available.saturating_sub(marker.len());
        let mut end = text_budget.min(line.len());
        while end > 0 && !line.is_char_boundary(end) {
            end -= 1;
        }
        output.push_str(&line[..end]);
        if marker.len() <= available.saturating_sub(end) {
            output.push_str(marker);
        }
    }
    output.push('\n');
    *line_count += 1;
    true
}

fn line_window_bounds(
    text: &str,
    start: usize,
    end: usize,
    context_lines: usize,
) -> (usize, usize) {
    let mut window_start = line_start(text, start.min(text.len()));
    for _ in 0..context_lines {
        let previous = previous_line_start(text, window_start);
        if previous == window_start {
            break;
        }
        window_start = previous;
    }

    let affected_byte = if end > start {
        end.saturating_sub(1)
    } else {
        start
    };
    let mut window_end = line_end(text, affected_byte.min(text.len()));
    for _ in 0..context_lines {
        let next = line_end(text, window_end);
        if next == window_end {
            break;
        }
        window_end = next;
    }
    (window_start, window_end)
}

fn line_start(text: &str, offset: usize) -> usize {
    let bytes = text.as_bytes();
    let mut index = offset.min(bytes.len());
    while index > 0 {
        if matches!(bytes[index - 1], b'\n' | b'\r') {
            return index;
        }
        index -= 1;
    }
    0
}

fn previous_line_start(text: &str, current_start: usize) -> usize {
    if current_start == 0 {
        return 0;
    }
    let bytes = text.as_bytes();
    let mut previous_end = current_start;
    if previous_end > 0 && bytes[previous_end - 1] == b'\n' {
        previous_end -= 1;
        if previous_end > 0 && bytes[previous_end - 1] == b'\r' {
            previous_end -= 1;
        }
    } else if previous_end > 0 && bytes[previous_end - 1] == b'\r' {
        previous_end -= 1;
    }
    line_start(text, previous_end)
}

fn line_end(text: &str, offset: usize) -> usize {
    let bytes = text.as_bytes();
    let mut index = offset.min(bytes.len());
    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => return index + 2,
            b'\r' | b'\n' => return index + 1,
            _ => index += 1,
        }
    }
    bytes.len()
}

fn line_number_at(text: &str, offset: usize) -> usize {
    let bytes = &text.as_bytes()[..offset.min(text.len())];
    let mut count = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
            count += 1;
            index += 2;
        } else if matches!(bytes[index], b'\r' | b'\n') {
            count += 1;
            index += 1;
        } else {
            index += 1;
        }
    }
    count.saturating_add(1)
}

fn normalize_to_lf(text: &str) -> Cow<'_, str> {
    if text.as_bytes().contains(&b'\r') {
        Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(text)
    }
}

#[derive(Clone, Copy)]
enum LineEnding {
    Lf,
    Crlf,
    Cr,
}

impl LineEnding {
    fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
            Self::Cr => "\r",
        }
    }
}

fn preserve_local_line_endings(
    normalized_replacement: &str,
    original: &str,
    raw_start: usize,
    raw_end: usize,
) -> String {
    if !normalized_replacement.contains('\n') {
        return normalized_replacement.to_owned();
    }

    let matched_endings = collect_line_endings(&original[raw_start..raw_end]);
    let fallback = matched_endings
        .last()
        .copied()
        .unwrap_or_else(|| nearby_line_ending(original, raw_start, raw_end));
    let mut ending_index = 0usize;
    let mut restored = String::with_capacity(normalized_replacement.len());
    for ch in normalized_replacement.chars() {
        if ch == '\n' {
            let ending = matched_endings
                .get(ending_index)
                .copied()
                .unwrap_or(fallback);
            restored.push_str(ending.as_str());
            ending_index += 1;
        } else {
            restored.push(ch);
        }
    }
    restored
}

fn collect_line_endings(text: &str) -> Vec<LineEnding> {
    let bytes = text.as_bytes();
    let mut endings = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                endings.push(LineEnding::Crlf);
                index += 2;
            }
            b'\r' => {
                endings.push(LineEnding::Cr);
                index += 1;
            }
            b'\n' => {
                endings.push(LineEnding::Lf);
                index += 1;
            }
            _ => index += 1,
        }
    }
    endings
}

fn nearby_line_ending(original: &str, raw_start: usize, raw_end: usize) -> LineEnding {
    let before = last_line_ending(&original[..raw_start]);
    let after = first_line_ending(&original[raw_end..]);
    match (before, after) {
        (Some((before_offset, ending)), Some((after_offset, after_ending))) => {
            let before_distance = raw_start.saturating_sub(before_offset);
            if before_distance <= after_offset {
                ending
            } else {
                after_ending
            }
        }
        (Some((_, ending)), None) | (None, Some((_, ending))) => ending,
        (None, None) => LineEnding::Lf,
    }
}

fn first_line_ending(text: &str) -> Option<(usize, LineEnding)> {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                return Some((index, LineEnding::Crlf));
            }
            b'\r' => return Some((index, LineEnding::Cr)),
            b'\n' => return Some((index, LineEnding::Lf)),
            _ => index += 1,
        }
    }
    None
}

fn last_line_ending(text: &str) -> Option<(usize, LineEnding)> {
    let bytes = text.as_bytes();
    let mut result = None;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                result = Some((index + 2, LineEnding::Crlf));
                index += 2;
            }
            b'\r' => {
                result = Some((index + 1, LineEnding::Cr));
                index += 1;
            }
            b'\n' => {
                result = Some((index + 1, LineEnding::Lf));
                index += 1;
            }
            _ => index += 1,
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn applies_multiple_edits_against_the_original_file() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        tokio::fs::write(&path, "alpha\nbeta\ngamma\n")
            .await
            .unwrap();
        EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "edits": [
                    { "oldText": "alpha", "newText": "one" },
                    { "oldText": "gamma", "newText": "three" }
                ]
            }))
            .await
            .unwrap();

        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "one\nbeta\nthree\n"
        );
    }

    #[tokio::test]
    async fn rejects_non_unique_matches() {
        let directory = tempdir().unwrap();
        tokio::fs::write(directory.path().join("file.txt"), "same same")
            .await
            .unwrap();
        let error = EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "edits": [{ "oldText": "same", "newText": "new" }]
            }))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("must be unique"));
    }

    #[tokio::test]
    async fn accepts_legacy_single_edit_arguments() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        tokio::fs::write(&path, "before").await.unwrap();
        EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "oldText": "before",
                "newText": "after"
            }))
            .await
            .unwrap();

        assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "after");
    }

    #[tokio::test]
    async fn preserves_bom_and_crlf() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        tokio::fs::write(&path, "\u{feff}one\r\ntwo\r\n")
            .await
            .unwrap();
        EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "edits": [{ "oldText": "one\ntwo", "newText": "first\nsecond" }]
            }))
            .await
            .unwrap();

        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "\u{feff}first\r\nsecond\r\n"
        );
    }

    #[tokio::test]
    async fn preserves_mixed_line_endings_outside_the_edit() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        tokio::fs::write(&path, "one\r\ntwo\nthree\r\nfour")
            .await
            .unwrap();

        EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "edits": [{ "oldText": "two", "newText": "second" }]
            }))
            .await
            .unwrap();

        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "one\r\nsecond\nthree\r\nfour"
        );
    }

    #[tokio::test]
    async fn preserves_each_matched_line_ending_in_multiline_replacements() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        tokio::fs::write(&path, "one\r\ntwo\nthree\r\nfour")
            .await
            .unwrap();

        EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "edits": [{
                    "oldText": "one\ntwo\nthree",
                    "newText": "first\nsecond\nthird"
                }]
            }))
            .await
            .unwrap();

        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "first\r\nsecond\nthird\r\nfour"
        );
    }

    #[tokio::test]
    async fn maps_multibyte_normalized_offsets_back_to_original_bytes() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        tokio::fs::write(&path, "前缀\r\n目标\n尾部").await.unwrap();

        EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "edits": [{
                    "oldText": "前缀\n目标",
                    "newText": "第一\n第二"
                }]
            }))
            .await
            .unwrap();

        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "第一\r\n第二\n尾部"
        );
    }

    #[test]
    fn normalized_text_borrows_lf_input_and_only_tracks_collapsed_crlf_offsets() {
        let lf = NormalizedText::new("alpha\nbeta");
        assert!(matches!(&lf.text, Cow::Borrowed(_)));
        assert!(lf.offset_adjustments.is_empty());

        let mixed = NormalizedText::new("a\r\né\rb");
        assert_eq!(mixed.text, "a\né\nb");
        assert_eq!(mixed.offset_adjustments, [(2, 1)]);
        assert_eq!(mixed.original_offset(1), 1);
        assert_eq!(mixed.original_offset(2), 3);
        assert_eq!(mixed.original_offset(4), 5);
        assert_eq!(mixed.original_offset(6), 7);
    }

    #[tokio::test]
    async fn replace_all_uses_the_original_snapshot_and_reports_structured_diff() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        tokio::fs::write(&path, "a b a").await.unwrap();

        let output = EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "edits": [
                    { "oldText": "a", "newText": "b", "replaceAll": true },
                    { "oldText": "b", "newText": "c" }
                ]
            }))
            .await
            .unwrap();

        assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "b c b");
        assert!(output.content.contains("replacing 3 occurrence(s)"));
        assert!(output.content.contains("--- a/file.txt"));
        assert!(output.content.contains("-a b a"));
        assert!(output.content.contains("+b c b"));
        assert_eq!(output.metadata.as_ref().unwrap()["replacementCount"], 3);
        assert_eq!(output.metadata.as_ref().unwrap()["editBlocks"], 2);
    }

    #[tokio::test]
    async fn legacy_single_edit_accepts_replace_all() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        tokio::fs::write(&path, "same same").await.unwrap();

        EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "oldText": "same",
                "newText": "new",
                "replaceAll": true
            }))
            .await
            .unwrap();

        assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "new new");
    }

    #[tokio::test]
    async fn rejects_overlaps_between_replace_all_and_other_edits() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        tokio::fs::write(&path, "one one").await.unwrap();

        let error = EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "edits": [
                    { "oldText": "one", "newText": "two", "replaceAll": true },
                    { "oldText": "one one", "newText": "all" }
                ]
            }))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("overlap"));
        assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "one one");
    }

    #[tokio::test]
    async fn accepts_one_unique_straight_to_curly_quote_fallback() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        tokio::fs::write(&path, "He said “hello”.").await.unwrap();

        EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "edits": [{ "oldText": "\"hello\"", "newText": "\"bye\"" }]
            }))
            .await
            .unwrap();

        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "He said \"bye\"."
        );
    }

    #[tokio::test]
    async fn rejects_ambiguous_curly_quote_fallback() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        tokio::fs::write(&path, "“same” and “same”").await.unwrap();

        let error = EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "edits": [{ "oldText": "\"same\"", "newText": "\"new\"" }]
            }))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("multiple straight/curly-quote"));
        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "“same” and “same”"
        );
    }

    #[tokio::test]
    async fn enforces_the_configured_file_size_limit_before_editing() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        tokio::fs::write(&path, "12345").await.unwrap();

        let error = EditTool::new(directory.path())
            .max_file_size(4)
            .execute(json!({
                "path": "file.txt",
                "edits": [{ "oldText": "1", "newText": "2" }]
            }))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("exceeding the 4-byte limit"));
        assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "12345");
    }

    #[tokio::test]
    async fn rejects_non_regular_files_before_reading() {
        let directory = tempdir().unwrap();
        let error = EditTool::new(directory.path())
            .execute(json!({
                "path": ".",
                "edits": [{ "oldText": "x", "newText": "y" }]
            }))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("not a regular file"));
    }

    #[tokio::test]
    async fn rejects_invalid_utf8_instead_of_rewriting_it_lossily() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("file.txt");
        tokio::fs::write(&path, [0xff, b'a']).await.unwrap();

        let error = EditTool::new(directory.path())
            .execute(json!({
                "path": "file.txt",
                "edits": [{ "oldText": "a", "newText": "b" }]
            }))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("not valid UTF-8"));
        assert_eq!(tokio::fs::read(path).await.unwrap(), [0xff, b'a']);
    }

    #[tokio::test]
    async fn missing_file_error_suggests_a_similar_sibling() {
        let directory = tempdir().unwrap();
        let candidate = directory.path().join("settings.rs");
        tokio::fs::write(&candidate, "before").await.unwrap();

        let error = EditTool::new(directory.path())
            .execute(json!({
                "path": "settings.ts",
                "edits": [{ "oldText": "before", "newText": "after" }]
            }))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("Did you mean"));
        assert!(error.to_string().contains("settings.rs"));
    }
}
