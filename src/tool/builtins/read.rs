use std::{
    collections::VecDeque,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;

use super::{
    common::{invalid_arguments, io_error, normalize_cwd, resolve_path_for_context},
    truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, TruncatedBy, truncate_head},
};
use crate::{
    error::ToolError,
    tool::{Tool, ToolEffect, ToolExecutionContext, ToolOutput},
    types::{ContentPart, Document, ImageUrl, ToolDefinition},
};

const FAST_PATH_MAX_BYTES: usize = 1024 * 1024;
const STREAM_BUFFER_BYTES: usize = 16 * 1024;
const UTF8_CAPTURE_LOOKAHEAD: usize = 4;
const DEFAULT_IMAGE_MAX_BYTES: usize = 20 * 1024 * 1024;
const DEFAULT_PDF_MAX_BYTES: usize = 50 * 1024 * 1024;
const DEFAULT_NOTEBOOK_MAX_BYTES: usize = 10 * 1024 * 1024;
const NOTEBOOK_RENDER_MAX_BYTES: usize = 4 * 1024 * 1024;
const NOTEBOOK_MAX_IMAGE_PARTS: usize = 16;
const READ_CACHE_CAPACITY: usize = 128;

#[derive(Clone, Debug)]
pub struct ReadTool {
    cwd: PathBuf,
    max_lines: usize,
    max_bytes: usize,
    max_image_bytes: usize,
    max_pdf_bytes: usize,
    max_notebook_bytes: usize,
    cache: Arc<Mutex<ReadCache>>,
}

impl ReadTool {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: normalize_cwd(cwd),
            max_lines: DEFAULT_MAX_LINES,
            max_bytes: DEFAULT_MAX_BYTES,
            max_image_bytes: DEFAULT_IMAGE_MAX_BYTES,
            max_pdf_bytes: DEFAULT_PDF_MAX_BYTES,
            max_notebook_bytes: DEFAULT_NOTEBOOK_MAX_BYTES,
            cache: Arc::new(Mutex::new(ReadCache::default())),
        }
    }

    pub fn output_limits(mut self, max_lines: usize, max_bytes: usize) -> Self {
        self.max_lines = max_lines.max(1);
        self.max_bytes = max_bytes.max(1);
        self
    }

    /// Sets the largest image accepted by this tool.
    pub fn image_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_image_bytes = max_bytes.max(1);
        self
    }

    /// Sets the PDF input limit. It is capped at 50 MiB, matching the most
    /// restrictive supported provider file-input limit.
    pub fn pdf_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_pdf_bytes = max_bytes.clamp(1, DEFAULT_PDF_MAX_BYTES);
        self
    }

    /// Sets the largest notebook JSON document accepted by this tool.
    pub fn notebook_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_notebook_bytes = max_bytes.max(1);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadKind {
    Text,
    Notebook,
    Image(ImageKind),
    Pdf,
}

impl ReadKind {
    fn metadata_name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Notebook => "notebook",
            Self::Image(_) => "image",
            Self::Pdf => "pdf",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImageKind {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl ImageKind {
    fn mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileFingerprint {
    len: u64,
    modified_nanos: Option<u128>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(unix)]
    mtime: i64,
    #[cfg(unix)]
    mtime_nsec: i64,
    #[cfg(unix)]
    ctime: i64,
    #[cfg(unix)]
    ctime_nsec: i64,
}

impl FileFingerprint {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt as _;

        Self {
            len: metadata.len(),
            modified_nanos: metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos()),
            #[cfg(unix)]
            dev: metadata.dev(),
            #[cfg(unix)]
            ino: metadata.ino(),
            #[cfg(unix)]
            mtime: metadata.mtime(),
            #[cfg(unix)]
            mtime_nsec: metadata.mtime_nsec(),
            #[cfg(unix)]
            ctime: metadata.ctime(),
            #[cfg(unix)]
            ctime_nsec: metadata.ctime_nsec(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReadCacheKey {
    path: PathBuf,
    offset: usize,
    limit: Option<usize>,
    kind: ReadKind,
    max_lines: usize,
    max_bytes: usize,
}

#[derive(Clone, Debug)]
struct ReadCacheEntry {
    key: ReadCacheKey,
    fingerprint: FileFingerprint,
    source_call_id: String,
    metadata: Value,
}

#[derive(Debug, Default)]
struct ReadCache {
    entries: VecDeque<ReadCacheEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArguments {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[async_trait]
impl Tool for ReadTool {
    fn effect(&self) -> ToolEffect {
        ToolEffect::ReadOnly
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "read",
            format!(
                "Read a regular file as UTF-8 text, a PNG/JPEG/GIF/WebP image, a PDF, or a Jupyter notebook. Text BOMs are hidden and CRLF/CR line endings are displayed as LF. Notebook ranges use cells; text ranges use lines. Text/notebook output is truncated to {} lines or {} bytes. Image/PDF signatures are verified before provider-neutral content blocks are returned. Use offset and limit for large text files and notebooks. FIFOs, devices, malformed media, and unsupported binary data are rejected.",
                self.max_lines, self.max_bytes,
            ),
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to read (relative to the configured working directory or absolute)"
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-indexed line number for text, or 1-indexed cell number for .ipynb; not valid for images/PDFs"
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Maximum number of lines for text, or cells for .ipynb; not valid for images/PDFs"
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        )
    }

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, ToolError> {
        self.execute_inner(arguments, None).await
    }

    async fn execute_with_context(
        &self,
        arguments: Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        self.execute_inner(arguments, Some(&context)).await
    }
}

impl ReadTool {
    async fn execute_inner(
        &self,
        arguments: Value,
        context: Option<&ToolExecutionContext>,
    ) -> Result<ToolOutput, ToolError> {
        let arguments: ReadArguments =
            serde_json::from_value(arguments).map_err(|error| invalid_arguments("read", error))?;
        let offset = arguments.offset.unwrap_or(1);
        if offset == 0 {
            return Err(ToolError::new("read offset must be at least 1"));
        }
        if arguments.limit == Some(0) {
            return Err(ToolError::new("read limit must be at least 1"));
        }

        let requested_path = resolve_path_for_context(&self.cwd, &arguments.path, context).await?;
        let metadata = match tokio::fs::metadata(&requested_path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(missing_file_error(&self.cwd, &requested_path).await);
            }
            Err(error) => return Err(io_error("could not inspect", &requested_path, error)),
        };
        if !metadata.is_file() {
            return Err(ToolError::new(format!(
                "could not read {}: path is not a regular file",
                requested_path.display()
            )));
        }
        let canonical_path = tokio::fs::canonicalize(&requested_path)
            .await
            .map_err(|error| io_error("could not canonicalize", &requested_path, error))?;
        let fingerprint = FileFingerprint::from_metadata(&metadata);
        let header = read_regular_prefix(&canonical_path, 16)
            .await
            .map_err(|error| io_error("could not read", &canonical_path, error))?;
        let kind = classify_read_kind(&canonical_path, &header)?;

        if matches!(kind, ReadKind::Image(_) | ReadKind::Pdf)
            && (arguments.offset.is_some() || arguments.limit.is_some())
        {
            return Err(ToolError::new(
                "read offset and limit apply only to text files and notebooks",
            ));
        }

        let cache_key = ReadCacheKey {
            path: canonical_path.clone(),
            offset,
            limit: arguments.limit,
            kind,
            max_lines: self.max_lines,
            max_bytes: self.max_bytes,
        };
        if let Some(context) = context
            && let Some(output) = self
                .cached_reference(&cache_key, &fingerprint, context)
                .await
        {
            return Ok(output);
        }

        let output = match kind {
            ReadKind::Text => {
                self.read_text(&canonical_path, offset, arguments.limit)
                    .await?
            }
            ReadKind::Notebook => {
                self.read_notebook(&canonical_path, offset, arguments.limit)
                    .await?
            }
            ReadKind::Image(image_kind) => self.read_image(&canonical_path, image_kind).await?,
            ReadKind::Pdf => self.read_pdf(&canonical_path).await?,
        };

        if let Some(context) = context
            && let Some(metadata) = output.metadata.clone()
            && let Ok(current) = tokio::fs::metadata(&canonical_path).await
            && FileFingerprint::from_metadata(&current) == fingerprint
        {
            self.remember_read(ReadCacheEntry {
                key: cache_key,
                fingerprint,
                source_call_id: context.call_id().to_owned(),
                metadata,
            })
            .await;
        }

        Ok(output)
    }

    async fn cached_reference(
        &self,
        key: &ReadCacheKey,
        fingerprint: &FileFingerprint,
        context: &ToolExecutionContext,
    ) -> Option<ToolOutput> {
        let mut cache = self.cache.lock().await;
        let index = cache.entries.iter().position(|entry| {
            entry.key == *key
                && entry.fingerprint == *fingerprint
                && entry.source_call_id != context.call_id()
                && context.has_visible_tool_result(&entry.source_call_id)
        })?;
        let entry = cache.entries.remove(index)?;
        let source_call_id = entry.source_call_id.clone();
        let mut metadata = entry.metadata.clone();
        if let Some(object) = metadata.as_object_mut() {
            object.insert("unchanged".to_owned(), Value::Bool(true));
            object.insert(
                "source_reference".to_owned(),
                json!({ "tool_call_id": source_call_id }),
            );
        }
        cache.entries.push_back(entry);
        Some(
            ToolOutput::success(format!(
                "[Unchanged from visible read result {source_call_id}: {}.]",
                key.path.display()
            ))
            .with_metadata(metadata),
        )
    }

    async fn remember_read(&self, entry: ReadCacheEntry) {
        let mut cache = self.cache.lock().await;
        cache.entries.retain(|existing| existing.key != entry.key);
        cache.entries.push_back(entry);
        while cache.entries.len() > READ_CACHE_CAPACITY {
            cache.entries.pop_front();
        }
    }

    async fn read_text(
        &self,
        path: &Path,
        offset: usize,
        limit: Option<usize>,
    ) -> Result<ToolOutput, ToolError> {
        let start = offset - 1;
        let requested_end = limit.map_or(usize::MAX, |limit| start.saturating_add(limit));
        let scanned = scan_text_file(path, start, requested_end, self.max_lines, self.max_bytes)
            .await
            .map_err(|error| io_error("could not read", path, error))?;

        if scanned.total_lines == Some(0) {
            return Ok(ToolOutput::success("[File is empty.]").with_metadata(json!({
                "kind": ReadKind::Text.metadata_name(),
                "path": path,
                "range": { "unit": "line", "offset": offset, "end": null, "limit": limit, "total": 0 },
                "truncated": false,
                "has_more": false,
            })));
        }
        if let Some(total_file_lines) = scanned.total_lines
            && start >= total_file_lines
        {
            return Err(ToolError::new(format!(
                "offset {offset} is beyond end of file ({total_file_lines} lines total)"
            )));
        }

        let selected = selected_text(&scanned.selected, start == 0)?;
        let truncated = truncate_head(&selected, self.max_lines, self.max_bytes);
        let start_line = start + 1;
        let shown_lines = if truncated.first_line_exceeds_limit {
            usize::from(scanned.selected_lines > 0)
        } else if truncated.truncated {
            truncated.output_lines
        } else {
            scanned.selected_lines
        };
        let end_line = (shown_lines > 0).then(|| start_line + shown_lines - 1);
        let has_more = truncated.truncated || scanned.has_more;
        let truncated_by = truncated.truncated_by.map(|reason| match reason {
            TruncatedBy::Lines => "lines",
            TruncatedBy::Bytes => "bytes",
        });

        let output = if truncated.first_line_exceeds_limit {
            let bom_bytes = if start == 0 && scanned.selected.starts_with(b"\xef\xbb\xbf") {
                3
            } else {
                0
            };
            long_line_preview(
                &selected,
                start_line,
                self.max_bytes,
                scanned.first_line_bytes.saturating_sub(bom_bytes),
                scanned.first_line_complete,
            )
        } else if truncated.truncated || scanned.has_more {
            let end_line = end_line.unwrap_or(start_line);
            let next_offset = end_line + 1;
            let qualifier = if truncated.truncated_by == Some(TruncatedBy::Bytes) {
                format!(" ({} byte limit)", self.max_bytes)
            } else {
                String::new()
            };
            let notice = scanned.total_lines.map_or_else(
                || {
                    format!(
                        "[Showing lines {start_line}-{end_line}{qualifier}; more content is available. Use offset={next_offset} to continue.]"
                    )
                },
                |total_file_lines| {
                    if !truncated.truncated
                        && !scanned.output_limit_reached
                        && end_line < total_file_lines
                    {
                        let remaining = total_file_lines - end_line;
                        format!(
                            "[{remaining} more lines in file. Use offset={next_offset} to continue.]"
                        )
                    } else {
                        format!(
                            "[Showing lines {start_line}-{end_line} of {total_file_lines}{qualifier}. Use offset={next_offset} to continue.]"
                        )
                    }
                },
            );
            content_with_notice(&truncated.content, notice)
        } else if selected.is_empty() && scanned.selected_lines == 1 {
            format!("[Line {start_line} is empty.]")
        } else {
            truncated.content
        };

        Ok(ToolOutput::success(output).with_metadata(json!({
            "kind": ReadKind::Text.metadata_name(),
            "path": path,
            "range": {
                "unit": "line",
                "offset": offset,
                "end": end_line,
                "limit": limit,
                "total": scanned.total_lines,
            },
            "truncated": truncated.truncated,
            "truncated_by": truncated_by,
            "has_more": has_more,
        })))
    }

    async fn read_image(
        &self,
        path: &Path,
        image_kind: ImageKind,
    ) -> Result<ToolOutput, ToolError> {
        let bytes = read_regular_file_bounded(path, self.max_image_bytes)
            .await
            .map_err(|error| io_error("could not read image", path, error))?;
        validate_image_bytes(image_kind, &bytes)?;
        let mime_type = image_kind.mime_type();
        Ok(ToolOutput::success(format!(
            "[Image: {} ({} bytes, {mime_type}).]",
            path.display(),
            bytes.len(),
        ))
        .with_content_part(ContentPart::image(ImageUrl::from_bytes(mime_type, &bytes)))
        .with_metadata(json!({
            "kind": ReadKind::Image(image_kind).metadata_name(),
            "path": path,
            "mime_type": mime_type,
            "bytes": bytes.len(),
            "range": null,
            "truncated": false,
            "has_more": false,
        })))
    }

    async fn read_pdf(&self, path: &Path) -> Result<ToolOutput, ToolError> {
        let bytes = read_regular_file_bounded(path, self.max_pdf_bytes)
            .await
            .map_err(|error| io_error("could not read PDF", path, error))?;
        if !bytes.starts_with(b"%PDF-") {
            return Err(ToolError::new(format!(
                "could not read {} as PDF: missing %PDF- file signature",
                path.display()
            )));
        }
        let filename = path.file_name().map_or_else(
            || "document.pdf".to_owned(),
            |name| name.to_string_lossy().into(),
        );
        Ok(ToolOutput::success(format!(
            "[PDF: {} ({} bytes).]",
            path.display(),
            bytes.len(),
        ))
        .with_content_part(ContentPart::document(Document::from_bytes(
            filename,
            "application/pdf",
            &bytes,
        )))
        .with_metadata(json!({
            "kind": ReadKind::Pdf.metadata_name(),
            "path": path,
            "mime_type": "application/pdf",
            "bytes": bytes.len(),
            "range": null,
            "truncated": false,
            "has_more": false,
        })))
    }

    async fn read_notebook(
        &self,
        path: &Path,
        offset: usize,
        limit: Option<usize>,
    ) -> Result<ToolOutput, ToolError> {
        let bytes = read_regular_file_bounded(path, self.max_notebook_bytes)
            .await
            .map_err(|error| io_error("could not read notebook", path, error))?;
        validate_utf8_text(&bytes)?;
        let text = selected_text(&bytes, true)?;
        let notebook: Value = serde_json::from_str(&text).map_err(|error| {
            ToolError::new(format!(
                "could not parse {} as a Jupyter notebook: {error}",
                path.display()
            ))
        })?;
        render_notebook(
            path,
            &notebook,
            offset,
            limit,
            self.max_lines,
            self.max_bytes,
            self.max_image_bytes,
        )
    }
}

fn expected_kind_from_extension(path: &Path) -> Option<ReadKind> {
    let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match extension.as_str() {
        "ipynb" => Some(ReadKind::Notebook),
        "png" => Some(ReadKind::Image(ImageKind::Png)),
        "jpg" | "jpeg" => Some(ReadKind::Image(ImageKind::Jpeg)),
        "gif" => Some(ReadKind::Image(ImageKind::Gif)),
        "webp" => Some(ReadKind::Image(ImageKind::Webp)),
        "pdf" => Some(ReadKind::Pdf),
        _ => None,
    }
}

fn detect_magic(bytes: &[u8]) -> Option<ReadKind> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ReadKind::Image(ImageKind::Png))
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(ReadKind::Image(ImageKind::Jpeg))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ReadKind::Image(ImageKind::Gif))
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ReadKind::Image(ImageKind::Webp))
    } else if bytes.starts_with(b"%PDF-") {
        Some(ReadKind::Pdf)
    } else {
        None
    }
}

fn kind_label(kind: ReadKind) -> &'static str {
    match kind {
        ReadKind::Text => "UTF-8 text",
        ReadKind::Notebook => "Jupyter notebook",
        ReadKind::Image(image) => image.mime_type(),
        ReadKind::Pdf => "application/pdf",
    }
}

fn classify_read_kind(path: &Path, header: &[u8]) -> Result<ReadKind, ToolError> {
    let expected = expected_kind_from_extension(path);
    if expected == Some(ReadKind::Notebook) {
        return Ok(ReadKind::Notebook);
    }
    let detected = detect_magic(header);
    if let Some(expected @ (ReadKind::Image(_) | ReadKind::Pdf)) = expected {
        if detected != Some(expected) {
            let actual = detected.map_or("unrecognized data", kind_label);
            return Err(ToolError::new(format!(
                "could not read {} as {}: file signature identifies {actual}",
                path.display(),
                kind_label(expected),
            )));
        }
        return Ok(expected);
    }
    Ok(detected.unwrap_or(ReadKind::Text))
}

async fn open_regular_file(path: &Path) -> io::Result<(tokio::fs::File, std::fs::Metadata)> {
    let metadata = tokio::fs::metadata(path).await?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is not a regular file",
        ));
    }
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path).await?;
    let opened_metadata = file.metadata().await?;
    if !opened_metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path changed and is no longer a regular file",
        ));
    }
    Ok((file, opened_metadata))
}

async fn read_regular_prefix(path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
    let (mut file, _) = open_regular_file(path).await?;
    let mut bytes = Vec::with_capacity(max_bytes);
    (&mut file)
        .take(max_bytes as u64)
        .read_to_end(&mut bytes)
        .await?;
    Ok(bytes)
}

async fn read_regular_file_bounded(path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
    let (mut file, metadata) = open_regular_file(path).await?;
    if metadata.len() > max_bytes as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "file is {} bytes, exceeding the configured {max_bytes} byte limit",
                metadata.len()
            ),
        ));
    }
    let mut bytes = Vec::with_capacity((metadata.len() as usize).min(max_bytes));
    (&mut file)
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("file grew beyond the configured {max_bytes} byte limit while reading"),
        ));
    }
    Ok(bytes)
}

fn validate_image_bytes(expected: ImageKind, bytes: &[u8]) -> Result<(), ToolError> {
    if detect_magic(bytes) == Some(ReadKind::Image(expected)) {
        Ok(())
    } else {
        Err(ToolError::new(format!(
            "image content does not match declared {} format",
            expected.mime_type()
        )))
    }
}

fn validate_utf8_text(bytes: &[u8]) -> Result<(), ToolError> {
    let mut validator = Utf8Validator::default();
    validator
        .consume(bytes, 0)
        .and_then(|()| validator.finish(bytes.len()))
        .map_err(|error| ToolError::new(error.to_string()))
}

#[derive(Debug)]
struct BoundedText {
    content: String,
    max_bytes: usize,
    truncated: bool,
}

impl BoundedText {
    fn new(max_bytes: usize) -> Self {
        Self {
            content: String::with_capacity(max_bytes.min(64 * 1024)),
            max_bytes,
            truncated: false,
        }
    }

    fn push(&mut self, text: &str) {
        if self.truncated || text.is_empty() {
            return;
        }
        let remaining = self.max_bytes.saturating_sub(self.content.len());
        if text.len() <= remaining {
            self.content.push_str(text);
            return;
        }
        let mut end = remaining.min(text.len());
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        self.content.push_str(&text[..end]);
        self.truncated = true;
    }
}

fn json_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let mut text = String::new();
            for part in parts {
                text.push_str(part.as_str()?);
            }
            Some(text)
        }
        _ => None,
    }
}

fn normalize_display_text(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn append_notebook_output(
    output: &Value,
    rendered: &mut BoundedText,
    parts: &mut Vec<ContentPart>,
    embedded_bytes: &mut usize,
    max_image_bytes: usize,
) {
    let output_type = output
        .get("output_type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match output_type {
        "stream" => {
            if let Some(text) = output.get("text").and_then(json_text) {
                let stream = output
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("stream");
                rendered.push(&format!("\nOutput ({stream}):\n"));
                rendered.push(normalize_display_text(&text).trim_end_matches('\n'));
            }
        }
        "execute_result" | "display_data" => {
            let Some(data) = output.get("data").and_then(Value::as_object) else {
                rendered.push("\n[Malformed notebook display output omitted.]");
                return;
            };
            if let Some(text) = data.get("text/plain").and_then(json_text) {
                rendered.push("\nOutput:\n");
                rendered.push(normalize_display_text(&text).trim_end_matches('\n'));
            }
            for (mime_type, image_kind) in [
                ("image/png", ImageKind::Png),
                ("image/jpeg", ImageKind::Jpeg),
            ] {
                let Some(encoded) = data.get(mime_type).and_then(json_text) else {
                    continue;
                };
                if parts.len() >= NOTEBOOK_MAX_IMAGE_PARTS {
                    rendered.push(&format!(
                        "\n[{mime_type} output omitted: attachment count limit reached.]"
                    ));
                    continue;
                }
                let compact = encoded
                    .chars()
                    .filter(|character| !character.is_ascii_whitespace())
                    .collect::<String>();
                let decoded = match STANDARD.decode(compact) {
                    Ok(decoded) => decoded,
                    Err(_) => {
                        rendered.push(&format!(
                            "\n[{mime_type} output omitted: invalid base64 data.]"
                        ));
                        continue;
                    }
                };
                if decoded.len() > max_image_bytes
                    || embedded_bytes.saturating_add(decoded.len()) > max_image_bytes
                {
                    rendered.push(&format!(
                        "\n[{mime_type} output omitted: embedded image byte limit reached.]"
                    ));
                    continue;
                }
                if validate_image_bytes(image_kind, &decoded).is_err() {
                    rendered.push(&format!(
                        "\n[{mime_type} output omitted: signature does not match MIME type.]"
                    ));
                    continue;
                }
                *embedded_bytes += decoded.len();
                parts.push(ContentPart::image(ImageUrl::from_bytes(
                    mime_type, &decoded,
                )));
                rendered.push(&format!("\n[{mime_type} output attached.]"));
            }
        }
        "error" => {
            let name = output
                .get("ename")
                .and_then(Value::as_str)
                .unwrap_or("Error");
            let value = output
                .get("evalue")
                .and_then(Value::as_str)
                .unwrap_or_default();
            rendered.push(&format!("\nError: {name}: {value}"));
            if let Some(traceback) = output.get("traceback").and_then(json_text) {
                rendered.push("\n");
                rendered.push(normalize_display_text(&traceback).trim_end_matches('\n'));
            }
        }
        _ => rendered.push(&format!("\n[Notebook output type {output_type} omitted.]")),
    }
}

fn render_notebook(
    path: &Path,
    notebook: &Value,
    offset: usize,
    limit: Option<usize>,
    max_lines: usize,
    max_bytes: usize,
    max_image_bytes: usize,
) -> Result<ToolOutput, ToolError> {
    if notebook.get("nbformat").and_then(Value::as_u64).is_none() {
        return Err(ToolError::new(format!(
            "could not parse {} as a Jupyter notebook: missing numeric nbformat",
            path.display()
        )));
    }
    let cells = notebook
        .get("cells")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ToolError::new(format!(
                "could not parse {} as a Jupyter notebook: missing cells array",
                path.display()
            ))
        })?;
    let total_cells = cells.len();
    let start = offset - 1;
    if total_cells == 0 {
        return Ok(ToolOutput::success("[Notebook has no cells.]").with_metadata(json!({
            "kind": ReadKind::Notebook.metadata_name(),
            "path": path,
            "range": { "unit": "cell", "offset": offset, "end": null, "limit": limit, "total": 0 },
            "truncated": false,
            "has_more": false,
        })));
    }
    if start >= total_cells {
        return Err(ToolError::new(format!(
            "offset {offset} is beyond end of notebook ({total_cells} cells total)"
        )));
    }
    let requested_end = limit
        .map_or(total_cells, |limit| start.saturating_add(limit))
        .min(total_cells);
    let render_limit = max_bytes
        .saturating_add(64 * 1024)
        .min(NOTEBOOK_RENDER_MAX_BYTES)
        .max(max_bytes.min(NOTEBOOK_RENDER_MAX_BYTES));
    let mut rendered = BoundedText::new(render_limit);
    let mut parts = Vec::new();
    let mut embedded_bytes = 0usize;
    let mut rendered_cells = 0usize;

    for (index, cell) in cells[start..requested_end].iter().enumerate() {
        let absolute_index = start + index;
        let cell_type = cell
            .get("cell_type")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::new(format!(
                    "could not parse {}: cell {} has no cell_type",
                    path.display(),
                    absolute_index + 1
                ))
            })?;
        let source = cell.get("source").and_then(json_text).ok_or_else(|| {
            ToolError::new(format!(
                "could not parse {}: cell {} has invalid source",
                path.display(),
                absolute_index + 1
            ))
        })?;
        if rendered_cells > 0 {
            rendered.push("\n\n");
        }
        let execution = cell
            .get("execution_count")
            .and_then(Value::as_u64)
            .map(|count| format!(", execution={count}"))
            .unwrap_or_default();
        rendered.push(&format!(
            "Cell {} [{cell_type}{execution}]\n",
            absolute_index + 1
        ));
        rendered.push(normalize_display_text(&source).trim_end_matches('\n'));
        if cell_type == "code"
            && let Some(outputs) = cell.get("outputs").and_then(Value::as_array)
        {
            for output in outputs {
                append_notebook_output(
                    output,
                    &mut rendered,
                    &mut parts,
                    &mut embedded_bytes,
                    max_image_bytes,
                );
                if rendered.truncated {
                    break;
                }
            }
        }
        rendered_cells += 1;
        if rendered.truncated {
            break;
        }
    }

    let hard_truncated = rendered.truncated;
    let truncation = truncate_head(&rendered.content, max_lines, max_bytes);
    let output_truncated = hard_truncated || truncation.truncated;
    let selected_end = start + rendered_cells;
    let has_more = output_truncated || selected_end < total_cells;
    let range_end = (rendered_cells > 0).then_some(selected_end);
    let embedded_image_count = parts.len();
    let mut content = truncation.content;
    if output_truncated {
        content = content_with_notice(
            &content,
            format!(
                "[Notebook output truncated at {max_lines} lines / {max_bytes} bytes. Use a later offset or a smaller cell range.]"
            ),
        );
    } else if selected_end < total_cells {
        let remaining = total_cells - selected_end;
        content = content_with_notice(
            &content,
            format!(
                "[{remaining} more notebook cells. Use offset={} to continue.]",
                selected_end + 1
            ),
        );
    }

    Ok(ToolOutput::success(content)
        .with_content_parts(parts)
        .with_metadata(json!({
            "kind": ReadKind::Notebook.metadata_name(),
            "path": path,
            "range": {
                "unit": "cell",
                "offset": offset,
                "end": range_end,
                "limit": limit,
                "total": total_cells,
            },
            "truncated": output_truncated,
            "truncated_by": if hard_truncated { Some("render_bytes") } else { truncation.truncated_by.map(|reason| match reason { TruncatedBy::Lines => "lines", TruncatedBy::Bytes => "bytes" }) },
            "has_more": has_more,
            "embedded_image_count": embedded_image_count,
            "embedded_image_bytes": embedded_bytes,
        })))
}

struct ScannedText {
    selected: Vec<u8>,
    total_lines: Option<usize>,
    selected_lines: usize,
    has_more: bool,
    output_limit_reached: bool,
    first_line_bytes: usize,
    first_line_complete: bool,
}

/// Uses one whole-file read for ordinary source files. Large files stream only
/// until the requested range plus proof of additional content is available;
/// their exact total line count is deliberately left unknown rather than
/// forcing every paged read to scan to EOF. Non-regular files are rejected both
/// before and after open so FIFOs and devices cannot block or produce infinity.
async fn scan_text_file(
    path: &Path,
    start: usize,
    requested_end: usize,
    max_lines: usize,
    max_bytes: usize,
) -> io::Result<ScannedText> {
    let metadata = tokio::fs::metadata(path).await?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is not a regular file",
        ));
    }

    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = options.open(path).await?;
    let opened_metadata = file.metadata().await?;
    if !opened_metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path changed and is no longer a regular file",
        ));
    }

    let mut scanner = TextRangeScanner::new(
        start,
        requested_end,
        max_lines,
        max_bytes,
        opened_metadata.len() > FAST_PATH_MAX_BYTES as u64,
    );
    let mut utf8 = Utf8Validator::default();
    let mut absolute_offset = 0usize;

    if opened_metadata.len() <= FAST_PATH_MAX_BYTES as u64 {
        // Read one byte beyond the fast-path boundary. A concurrently growing
        // file then falls back to streaming without ever allocating its full
        // new size or reopening it.
        let mut bytes = Vec::with_capacity(opened_metadata.len() as usize + 1);
        (&mut file)
            .take((FAST_PATH_MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .await?;
        utf8.consume(&bytes, absolute_offset)?;
        absolute_offset = absolute_offset.saturating_add(bytes.len());
        scanner.consume(&bytes);
        if bytes.len() <= FAST_PATH_MAX_BYTES {
            utf8.finish(absolute_offset)?;
            return Ok(scanner.finish());
        }
        scanner.enable_early_stop();
        if scanner.stopped {
            return Ok(scanner.into_early_result());
        }
    }

    let mut buffer = [0u8; STREAM_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            utf8.finish(absolute_offset)?;
            return Ok(scanner.finish());
        }
        utf8.consume(&buffer[..read], absolute_offset)?;
        absolute_offset = absolute_offset.saturating_add(read);
        scanner.consume(&buffer[..read]);
        if scanner.stopped {
            return Ok(scanner.into_early_result());
        }
    }
}

struct TextRangeScanner {
    start: usize,
    requested_end: usize,
    max_lines: usize,
    capture_limit: usize,
    allow_early_stop: bool,
    selected: Vec<u8>,
    selected_lines: usize,
    current_line: usize,
    current_line_exists: bool,
    capturing_current_line: bool,
    pending_cr: bool,
    capture_exhausted: bool,
    has_more: bool,
    output_limit_reached: bool,
    stopped: bool,
    first_line_bytes: usize,
    first_line_complete: bool,
}

impl TextRangeScanner {
    fn new(
        start: usize,
        requested_end: usize,
        max_lines: usize,
        max_bytes: usize,
        allow_early_stop: bool,
    ) -> Self {
        let capture_limit = max_bytes.saturating_add(UTF8_CAPTURE_LOOKAHEAD);
        Self {
            start,
            requested_end,
            max_lines,
            capture_limit,
            allow_early_stop,
            selected: Vec::with_capacity(capture_limit.min(64 * 1024)),
            selected_lines: 0,
            current_line: 0,
            current_line_exists: false,
            capturing_current_line: false,
            pending_cr: false,
            capture_exhausted: false,
            has_more: false,
            output_limit_reached: false,
            stopped: false,
            first_line_bytes: 0,
            first_line_complete: false,
        }
    }

    fn enable_early_stop(&mut self) {
        self.allow_early_stop = true;
        if self.has_more {
            self.stopped = true;
        }
    }

    fn consume(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.stopped {
                break;
            }
            if self.pending_cr {
                self.pending_cr = false;
                self.finish_line();
                if self.stopped {
                    break;
                }
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\r' => self.pending_cr = true,
                b'\n' => self.finish_line(),
                _ => self.content_byte(byte),
            }
        }
    }

    fn content_byte(&mut self, byte: u8) {
        self.establish_line();
        if self.current_line == self.start {
            self.first_line_bytes = self.first_line_bytes.saturating_add(1);
        }
        if self.stopped || !self.capturing_current_line {
            return;
        }
        self.capture(byte);
    }

    fn finish_line(&mut self) {
        self.establish_line();
        if self.current_line == self.start {
            self.first_line_complete = true;
        }
        if self.stopped {
            return;
        }
        self.current_line = self.current_line.saturating_add(1);
        self.current_line_exists = false;
        self.capturing_current_line = false;
    }

    fn establish_line(&mut self) {
        if self.current_line_exists {
            return;
        }
        self.current_line_exists = true;
        if self.current_line < self.start {
            return;
        }
        if self.current_line >= self.requested_end {
            self.omit_content();
            return;
        }
        if self.selected_lines >= self.max_lines || self.capture_exhausted {
            self.output_limit_reached = true;
            self.omit_content();
            return;
        }
        if self.selected_lines > 0 && !self.capture(b'\n') {
            return;
        }
        self.selected_lines = self.selected_lines.saturating_add(1);
        self.capturing_current_line = true;
    }

    fn capture(&mut self, byte: u8) -> bool {
        if self.selected.len() >= self.capture_limit {
            self.capture_exhausted = true;
            self.output_limit_reached = true;
            self.capturing_current_line = false;
            self.omit_content();
            return false;
        }
        self.selected.push(byte);
        true
    }

    fn omit_content(&mut self) {
        self.has_more = true;
        if self.allow_early_stop {
            self.stopped = true;
        }
    }

    fn finish(mut self) -> ScannedText {
        if self.pending_cr {
            self.pending_cr = false;
            self.finish_line();
        }
        let total_lines = self
            .current_line
            .saturating_add(usize::from(self.current_line_exists));
        let first_line_complete = self.first_line_complete
            || self.current_line > self.start
            || (self.current_line == self.start && self.current_line_exists);
        ScannedText {
            selected: self.selected,
            total_lines: Some(total_lines),
            selected_lines: self.selected_lines,
            has_more: self.has_more,
            output_limit_reached: self.output_limit_reached,
            first_line_bytes: self.first_line_bytes,
            first_line_complete,
        }
    }

    fn into_early_result(self) -> ScannedText {
        ScannedText {
            selected: self.selected,
            total_lines: None,
            selected_lines: self.selected_lines,
            has_more: true,
            output_limit_reached: self.output_limit_reached,
            first_line_bytes: self.first_line_bytes,
            first_line_complete: self.first_line_complete,
        }
    }
}

#[derive(Default)]
struct Utf8Validator {
    pending: [u8; 4],
    pending_len: usize,
    pending_offset: usize,
}

impl Utf8Validator {
    fn consume(&mut self, bytes: &[u8], absolute_offset: usize) -> io::Result<()> {
        if let Some(index) = bytes.iter().position(|byte| *byte == 0) {
            return Err(binary_text_error(format!(
                "NUL byte at byte {}",
                absolute_offset + index + 1
            )));
        }

        let mut index = 0usize;
        if self.pending_len > 0 {
            let width = utf8_width(self.pending[0]).expect("pending UTF-8 starts with a lead byte");
            let needed = width - self.pending_len;
            let copied = needed.min(bytes.len());
            self.pending[self.pending_len..self.pending_len + copied]
                .copy_from_slice(&bytes[..copied]);
            self.pending_len += copied;
            index += copied;
            if self.pending_len < width {
                return Ok(());
            }
            if std::str::from_utf8(&self.pending[..width]).is_err() {
                return Err(binary_text_error(format!(
                    "invalid UTF-8 at byte {}",
                    self.pending_offset + 1
                )));
            }
            self.pending_len = 0;
        }

        let remaining = &bytes[index..];
        match std::str::from_utf8(remaining) {
            Ok(_) => Ok(()),
            Err(error) if error.error_len().is_some() => Err(binary_text_error(format!(
                "invalid UTF-8 at byte {}",
                absolute_offset + index + error.valid_up_to() + 1
            ))),
            Err(error) => {
                let suffix = &remaining[error.valid_up_to()..];
                debug_assert!(suffix.len() <= 3);
                self.pending[..suffix.len()].copy_from_slice(suffix);
                self.pending_len = suffix.len();
                self.pending_offset = absolute_offset + index + error.valid_up_to();
                Ok(())
            }
        }
    }

    fn finish(&self, absolute_offset: usize) -> io::Result<()> {
        if self.pending_len == 0 {
            Ok(())
        } else {
            Err(binary_text_error(format!(
                "incomplete UTF-8 sequence at byte {}",
                absolute_offset - self.pending_len + 1
            )))
        }
    }
}

fn utf8_width(byte: u8) -> Option<usize> {
    match byte {
        0xC2..=0xDF => Some(2),
        0xE0..=0xEF => Some(3),
        0xF0..=0xF4 => Some(4),
        _ => None,
    }
}

fn binary_text_error(detail: String) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("file is not UTF-8 text ({detail}); use a binary-aware tool such as file or xxd"),
    )
}

fn selected_text(bytes: &[u8], strip_bom: bool) -> Result<String, ToolError> {
    let valid_len = match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        Err(error) => {
            return Err(ToolError::new(format!(
                "read captured invalid UTF-8 at byte {}",
                error.valid_up_to() + 1
            )));
        }
    };
    let text = std::str::from_utf8(&bytes[..valid_len])
        .expect("the validated UTF-8 prefix must remain valid");
    Ok(if strip_bom {
        text.strip_prefix('\u{feff}').unwrap_or(text).to_owned()
    } else {
        text.to_owned()
    })
}

fn long_line_preview(
    selected: &str,
    line: usize,
    max_bytes: usize,
    observed_line_bytes: usize,
    line_complete: bool,
) -> String {
    let first_line = selected.split('\n').next().unwrap_or_default();
    let mut preview_end = first_line.len().min(max_bytes);
    while preview_end > 0 && !first_line.is_char_boundary(preview_end) {
        preview_end -= 1;
    }
    let preview = &first_line[..preview_end];
    let preview_chars = preview.chars().count();
    let size = if line_complete {
        format!("is {observed_line_bytes} bytes")
    } else {
        format!("is at least {observed_line_bytes} bytes")
    };
    let notice = format!(
        "[Line {line} {size} and exceeds the {max_bytes} byte limit. Showing UTF-8 bytes 1-{preview_end} (characters 1-{preview_chars}). Continue this line at byte {} / character {} with a byte- or column-range reader.]",
        preview_end + 1,
        preview_chars + 1,
    );
    content_with_notice(preview, notice)
}

fn content_with_notice(content: &str, notice: String) -> String {
    if content.is_empty() {
        notice
    } else {
        format!("{content}\n\n{notice}")
    }
}

async fn missing_file_error(cwd: &Path, path: &Path) -> ToolError {
    let mut message = format!(
        "could not read {}: file does not exist (working directory: {})",
        path.display(),
        cwd.display()
    );
    if let Some(suggestion) = find_similar_path(path).await {
        message.push_str(&format!(". Did you mean {}?", suggestion.display()));
    }
    ToolError::new(message)
}

async fn find_similar_path(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let wanted = path.file_name()?.to_string_lossy().to_lowercase();
    let wanted_chars = wanted.chars().count();
    let threshold = match wanted_chars {
        0..=4 => 1,
        5..=10 => 2,
        _ => 3,
    };
    let mut entries = tokio::fs::read_dir(parent).await.ok()?;
    let mut best: Option<(usize, PathBuf)> = None;
    for _ in 0..512 {
        let Some(entry) = entries.next_entry().await.ok()? else {
            break;
        };
        let candidate = entry.file_name().to_string_lossy().to_lowercase();
        let distance = edit_distance(&wanted, &candidate);
        if best
            .as_ref()
            .is_none_or(|(best_distance, _)| distance < *best_distance)
        {
            best = Some((distance, entry.path()));
            if distance == 0 {
                break;
            }
        }
    }
    best.and_then(|(distance, candidate)| (distance <= threshold).then_some(candidate))
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    let mut current = vec![0; right.len() + 1];
    for (left_index, left_char) in left.chars().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_char) in right.iter().enumerate() {
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(previous[right_index] + usize::from(left_char != *right_char));
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, sync::Arc};

    use tempfile::tempdir;

    use super::*;
    use crate::tool::ToolCancellation;

    #[tokio::test]
    async fn reads_ranges_and_reports_continuation() {
        let directory = tempdir().unwrap();
        tokio::fs::write(directory.path().join("notes.txt"), "one\ntwo\nthree")
            .await
            .unwrap();
        let output = ReadTool::new(directory.path())
            .execute(json!({ "path": "notes.txt", "offset": 2, "limit": 1 }))
            .await
            .unwrap();

        assert_eq!(
            output.content,
            "two\n\n[1 more lines in file. Use offset=3 to continue.]"
        );
    }

    #[tokio::test]
    async fn truncates_at_complete_lines() {
        let directory = tempdir().unwrap();
        tokio::fs::write(directory.path().join("notes.txt"), "one\ntwo\nthree")
            .await
            .unwrap();
        let output = ReadTool::new(directory.path())
            .output_limits(2, 100)
            .execute(json!({ "path": "notes.txt" }))
            .await
            .unwrap();

        assert!(output.content.starts_with("one\ntwo\n\n[Showing lines 1-2"));
    }

    #[tokio::test]
    async fn streams_large_lines_with_bounded_capture() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("large.txt");
        tokio::fs::write(&path, vec![b'x'; FAST_PATH_MAX_BYTES + 1024])
            .await
            .unwrap();

        let scanned = scan_text_file(&path, 0, usize::MAX, 2, 32).await.unwrap();
        assert_eq!(scanned.selected.len(), 32 + UTF8_CAPTURE_LOOKAHEAD);
        assert_eq!(scanned.total_lines, None);
        assert!(scanned.first_line_bytes <= 32 + UTF8_CAPTURE_LOOKAHEAD + 1);

        let output = ReadTool::new(directory.path())
            .output_limits(2, 32)
            .execute(json!({ "path": "large.txt" }))
            .await
            .unwrap();
        assert!(output.content.starts_with(&"x".repeat(32)));
        assert!(output.content.contains("is at least"));
        assert!(output.content.contains("byte 33 / character 33"));
    }

    #[tokio::test]
    async fn ranges_use_text_line_semantics_without_a_phantom_trailing_line() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("ranges.txt");
        for content in ["one", "one\n", "\n\n", "一\ntwo\nthree"] {
            tokio::fs::write(&path, content).await.unwrap();
            let lines = content.split_terminator('\n').collect::<Vec<_>>();
            for start in 0..lines.len() {
                for limit in 1..=lines.len() + 1 {
                    let end = start.saturating_add(limit).min(lines.len());
                    let scanned = scan_text_file(&path, start, start + limit, 100, 1024)
                        .await
                        .unwrap();
                    assert_eq!(scanned.total_lines, Some(lines.len()));
                    assert_eq!(
                        selected_text(&scanned.selected, start == 0).unwrap(),
                        lines[start..end].join("\n")
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn empty_file_is_reported_without_inventing_a_line() {
        let directory = tempdir().unwrap();
        tokio::fs::write(directory.path().join("empty.txt"), "")
            .await
            .unwrap();

        let output = ReadTool::new(directory.path())
            .execute(json!({ "path": "empty.txt" }))
            .await
            .unwrap();

        assert_eq!(output.content, "[File is empty.]");
    }

    #[tokio::test]
    async fn strips_bom_and_normalizes_crlf_and_cr_line_endings() {
        let directory = tempdir().unwrap();
        tokio::fs::write(
            directory.path().join("mixed.txt"),
            b"\xef\xbb\xbfone\r\ntwo\rthree\n",
        )
        .await
        .unwrap();

        let output = ReadTool::new(directory.path())
            .execute(json!({ "path": "mixed.txt" }))
            .await
            .unwrap();

        assert_eq!(output.content, "one\ntwo\nthree");
    }

    #[tokio::test]
    async fn rejects_invalid_utf8_and_nul_bytes() {
        let directory = tempdir().unwrap();
        let invalid = directory.path().join("invalid.txt");
        tokio::fs::write(&invalid, b"valid\xffinvalid")
            .await
            .unwrap();
        let error = ReadTool::new(directory.path())
            .execute(json!({ "path": "invalid.txt" }))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("invalid UTF-8"));

        let nul = directory.path().join("nul.txt");
        tokio::fs::write(&nul, b"valid\0binary").await.unwrap();
        let error = ReadTool::new(directory.path())
            .execute(json!({ "path": "nul.txt" }))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("NUL byte"));
    }

    #[tokio::test]
    async fn validates_utf8_sequences_split_across_stream_buffers() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("split-utf8.txt");
        let mut bytes = vec![b'a'; STREAM_BUFFER_BYTES - 1];
        bytes.extend_from_slice("界\nnext".as_bytes());
        bytes.resize(FAST_PATH_MAX_BYTES + 1, b'x');
        tokio::fs::write(&path, bytes).await.unwrap();

        let scanned = scan_text_file(&path, 0, 1, 10, FAST_PATH_MAX_BYTES + 10)
            .await
            .unwrap();

        assert_eq!(scanned.total_lines, None);
        assert!(
            selected_text(&scanned.selected, true)
                .unwrap()
                .ends_with('界')
        );
    }

    #[tokio::test]
    async fn long_multibyte_line_preview_stops_on_utf8_boundary() {
        let directory = tempdir().unwrap();
        tokio::fs::write(directory.path().join("unicode.txt"), "é".repeat(100))
            .await
            .unwrap();

        let output = ReadTool::new(directory.path())
            .output_limits(10, 5)
            .execute(json!({ "path": "unicode.txt" }))
            .await
            .unwrap();

        assert!(output.content.starts_with("éé\n\n"));
        assert!(!output.content.contains('\u{fffd}'));
        assert!(output.content.contains("byte 5 / character 3"));
    }

    #[tokio::test]
    async fn large_files_stop_after_proving_more_content_exists() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("large.txt");
        let mut content = b"one\ntwo\n".to_vec();
        content.resize(FAST_PATH_MAX_BYTES + 1, b'x');
        tokio::fs::write(&path, content).await.unwrap();

        let scanned = scan_text_file(&path, 0, 1, 100, 1024).await.unwrap();
        assert_eq!(scanned.total_lines, None);
        assert!(scanned.has_more);
        assert_eq!(selected_text(&scanned.selected, true).unwrap(), "one");

        let output = ReadTool::new(directory.path())
            .execute(json!({ "path": "large.txt", "limit": 1 }))
            .await
            .unwrap();
        assert!(output.content.contains("more content is available"));
        assert!(!output.content.contains(" of "));
    }

    #[tokio::test]
    async fn missing_file_error_suggests_a_similar_name() {
        let directory = tempdir().unwrap();
        tokio::fs::write(directory.path().join("notes.txt"), "notes")
            .await
            .unwrap();

        let error = ReadTool::new(directory.path())
            .execute(json!({ "path": "notse.txt" }))
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("file does not exist"));
        assert!(message.contains("Did you mean"));
        assert!(message.contains("notes.txt"));
    }

    #[tokio::test]
    async fn returns_verified_images_as_content_blocks() {
        let directory = tempdir().unwrap();
        let bytes = b"\x89PNG\r\n\x1a\nminimal";
        tokio::fs::write(directory.path().join("plot.png"), bytes)
            .await
            .unwrap();

        let output = ReadTool::new(directory.path())
            .execute(json!({ "path": "plot.png" }))
            .await
            .unwrap();

        assert!(matches!(
            output.content_parts.as_slice(),
            [ContentPart::ImageUrl { image_url }]
                if image_url.url.starts_with("data:image/png;base64,")
        ));
        assert_eq!(output.metadata.as_ref().unwrap()["kind"], "image");
        assert_eq!(output.metadata.as_ref().unwrap()["truncated"], false);
    }

    #[tokio::test]
    async fn rejects_media_extension_and_magic_mismatches() {
        let directory = tempdir().unwrap();
        tokio::fs::write(directory.path().join("fake.png"), b"plain text")
            .await
            .unwrap();

        let error = ReadTool::new(directory.path())
            .execute(json!({ "path": "fake.png" }))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("file signature"));
        assert!(error.to_string().contains("image/png"));
    }

    #[tokio::test]
    async fn returns_verified_pdfs_and_enforces_the_configured_limit() {
        let directory = tempdir().unwrap();
        let bytes = b"%PDF-1.7\n%%EOF\n";
        tokio::fs::write(directory.path().join("paper.pdf"), bytes)
            .await
            .unwrap();

        let output = ReadTool::new(directory.path())
            .execute(json!({ "path": "paper.pdf" }))
            .await
            .unwrap();
        assert!(matches!(
            output.content_parts.as_slice(),
            [ContentPart::Document { document }]
                if document.mime_type == "application/pdf"
                    && document.data.starts_with("data:application/pdf;base64,")
        ));

        let error = ReadTool::new(directory.path())
            .pdf_max_bytes(5)
            .execute(json!({ "path": "paper.pdf" }))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("5 byte limit"));
    }

    #[tokio::test]
    async fn renders_notebook_cells_outputs_and_embedded_images() {
        let directory = tempdir().unwrap();
        let png = STANDARD.encode(b"\x89PNG\r\n\x1a\nminimal");
        let notebook = json!({
            "nbformat": 4,
            "nbformat_minor": 5,
            "cells": [
                { "cell_type": "markdown", "metadata": {}, "source": ["# Title\n"] },
                {
                    "cell_type": "code",
                    "metadata": {},
                    "execution_count": 7,
                    "source": ["print('ok')\n"],
                    "outputs": [
                        { "output_type": "stream", "name": "stdout", "text": ["ok\n"] },
                        { "output_type": "display_data", "metadata": {}, "data": { "image/png": png } }
                    ]
                }
            ]
        });
        tokio::fs::write(
            directory.path().join("analysis.ipynb"),
            serde_json::to_vec(&notebook).unwrap(),
        )
        .await
        .unwrap();

        let output = ReadTool::new(directory.path())
            .execute(json!({ "path": "analysis.ipynb", "offset": 2, "limit": 1 }))
            .await
            .unwrap();

        assert!(output.content.contains("Cell 2 [code, execution=7]"));
        assert!(output.content.contains("Output (stdout):\nok"));
        assert!(matches!(
            output.content_parts.as_slice(),
            [ContentPart::ImageUrl { image_url }]
                if image_url.url.starts_with("data:image/png;base64,")
        ));
        let metadata = output.metadata.unwrap();
        assert_eq!(metadata["range"]["unit"], "cell");
        assert_eq!(metadata["range"]["offset"], 2);
        assert_eq!(metadata["has_more"], false);
    }

    #[tokio::test]
    async fn notebook_ranges_and_render_limits_report_continuation() {
        let directory = tempdir().unwrap();
        let notebook = json!({
            "nbformat": 4,
            "cells": [
                { "cell_type": "markdown", "source": ["first"] },
                { "cell_type": "markdown", "source": ["second"] }
            ]
        });
        tokio::fs::write(
            directory.path().join("small.ipynb"),
            serde_json::to_vec(&notebook).unwrap(),
        )
        .await
        .unwrap();

        let output = ReadTool::new(directory.path())
            .execute(json!({ "path": "small.ipynb", "limit": 1 }))
            .await
            .unwrap();
        assert!(output.content.contains("1 more notebook cells"));
        assert_eq!(output.metadata.as_ref().unwrap()["has_more"], true);

        let output = ReadTool::new(directory.path())
            .output_limits(1, 8)
            .execute(json!({ "path": "small.ipynb" }))
            .await
            .unwrap();
        assert_eq!(output.metadata.as_ref().unwrap()["truncated"], true);
        assert!(output.content.contains("Notebook output truncated"));
    }

    #[tokio::test]
    async fn deduplicates_only_against_visible_unchanged_results() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("notes.txt");
        tokio::fs::write(&path, "one\ntwo").await.unwrap();
        let tool = ReadTool::new(directory.path());

        let first = tool
            .execute_with_context(
                json!({ "path": "./notes.txt", "limit": 1 }),
                ToolExecutionContext::new(
                    "read-1",
                    ToolCancellation::new(),
                    Arc::new(HashSet::new()),
                    None,
                ),
            )
            .await
            .unwrap();
        assert!(first.content.starts_with("one"));

        let second = tool
            .execute_with_context(
                json!({ "path": path, "limit": 1 }),
                ToolExecutionContext::new(
                    "read-2",
                    ToolCancellation::new(),
                    Arc::new(HashSet::from(["read-1".to_owned()])),
                    None,
                ),
            )
            .await
            .unwrap();
        assert!(
            second
                .content
                .contains("Unchanged from visible read result read-1")
        );
        assert_eq!(
            second.metadata.as_ref().unwrap()["source_reference"]["tool_call_id"],
            "read-1"
        );

        let no_source = tool
            .execute_with_context(
                json!({ "path": "notes.txt", "limit": 1 }),
                ToolExecutionContext::new(
                    "read-3",
                    ToolCancellation::new(),
                    Arc::new(HashSet::new()),
                    None,
                ),
            )
            .await
            .unwrap();
        assert!(no_source.content.starts_with("one"));
        assert!(!no_source.content.contains("Unchanged"));

        tokio::fs::write(&path, "changed\ntwo\nthree")
            .await
            .unwrap();
        let changed = tool
            .execute_with_context(
                json!({ "path": "notes.txt", "limit": 1 }),
                ToolExecutionContext::new(
                    "read-4",
                    ToolCancellation::new(),
                    Arc::new(HashSet::from(["read-3".to_owned()])),
                    None,
                ),
            )
            .await
            .unwrap();
        assert!(changed.content.starts_with("changed"));
        assert!(!changed.content.contains("Unchanged"));

        // Calling the compatibility entry point never returns a reference.
        let direct = tool
            .execute(json!({ "path": "notes.txt", "limit": 1 }))
            .await
            .unwrap();
        assert!(direct.content.starts_with("changed"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_fifo_without_opening_it() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt as _;

        let directory = tempdir().unwrap();
        let path = directory.path().join("pipe");
        let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        let error = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            ReadTool::new(directory.path()).execute(json!({ "path": "pipe" })),
        )
        .await
        .expect("FIFO validation must not block")
        .unwrap_err();
        assert!(error.to_string().contains("not a regular file"));
    }
}
