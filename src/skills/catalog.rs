use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value as YamlValue};
use thiserror::Error;

use crate::types::{Content, ContentPart};

use super::{DuplicateSkillPolicy, SkillDirectory, SkillsConfig};

const MAX_LISTING_DESCRIPTION_CHARS: usize = 250;
const MIN_LISTING_DESCRIPTION_CHARS: usize = 20;
const SKILL_FILE_NAME: &str = "SKILL.md";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLevel {
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SkillDiagnostic {
    pub level: DiagnosticLevel,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub message: String,
}

impl SkillDiagnostic {
    fn warning(code: &str, path: impl Into<Option<PathBuf>>, message: impl Into<String>) -> Self {
        Self {
            level: DiagnosticLevel::Warning,
            code: code.to_owned(),
            path: path.into(),
            message: message.into(),
        }
    }

    fn error(code: &str, path: impl Into<Option<PathBuf>>, message: impl Into<String>) -> Self {
        Self {
            level: DiagnosticLevel::Error,
            code: code.to_owned(),
            path: path.into(),
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argument_names: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub model_invocable: bool,
    pub user_invocable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillInvocation {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

impl SkillInvocation {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            arguments: None,
        }
    }

    pub fn arguments(mut self, arguments: impl Into<String>) -> Self {
        self.arguments = Some(arguments.into());
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedSkill {
    pub name: String,
    pub content: String,
}

#[derive(Clone)]
struct LoadedSkill {
    metadata: SkillMetadata,
    body: String,
    base_dir: PathBuf,
}

#[derive(Default)]
struct CatalogInner {
    by_name: BTreeMap<String, Arc<LoadedSkill>>,
    metadata: Vec<SkillMetadata>,
    diagnostics: Vec<SkillDiagnostic>,
    model_listing: String,
    listing_char_budget: usize,
}

/// An immutable, session-safe snapshot of discovered skills.
#[derive(Clone, Default)]
pub struct SkillCatalog {
    inner: Arc<CatalogInner>,
}

impl fmt::Debug for SkillCatalog {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillCatalog")
            .field("skills", &self.inner.metadata.len())
            .field("diagnostics", &self.inner.diagnostics.len())
            .finish()
    }
}

impl SkillCatalog {
    pub async fn load(config: &SkillsConfig) -> Result<Self, SkillError> {
        if !config.enabled {
            return Ok(Self::default());
        }

        let mut by_name = BTreeMap::<String, Arc<LoadedSkill>>::new();
        let mut diagnostics = Vec::new();
        let mut canonical_roots = HashSet::new();
        let mut loaded_files = 0usize;

        for directory in &config.directories {
            let Some(root) = prepare_root(directory, config, &mut diagnostics).await? else {
                continue;
            };
            if !canonical_roots.insert(root.clone()) {
                diagnostics.push(SkillDiagnostic::warning(
                    "duplicate_directory",
                    Some(directory.path.clone()),
                    "skill directory resolves to a directory that was already scanned",
                ));
                continue;
            }

            let entries = match read_sorted_entries(&directory.path).await {
                Ok(entries) => entries,
                Err(error) => {
                    recover_or_fail(
                        config,
                        directory.required,
                        &mut diagnostics,
                        "read_directory_failed",
                        &directory.path,
                        format!("could not scan skill directory: {error}"),
                    )?;
                    continue;
                }
            };

            for entry in entries {
                let path = entry.path();
                let file_type = match entry.file_type().await {
                    Ok(file_type) => file_type,
                    Err(error) => {
                        recover_or_fail(
                            config,
                            false,
                            &mut diagnostics,
                            "inspect_entry_failed",
                            &path,
                            format!("could not inspect skill directory entry: {error}"),
                        )?;
                        continue;
                    }
                };
                if file_type.is_symlink() && !config.follow_symlinks {
                    diagnostics.push(SkillDiagnostic::warning(
                        "symlink_ignored",
                        Some(path),
                        "symbolic-link skill directory ignored",
                    ));
                    continue;
                }
                let is_directory = if file_type.is_dir() {
                    true
                } else if file_type.is_symlink() {
                    tokio::fs::metadata(&path)
                        .await
                        .map(|metadata| metadata.is_dir())
                        .unwrap_or(false)
                } else {
                    false
                };
                if !is_directory {
                    continue;
                }

                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    diagnostics.push(SkillDiagnostic::warning(
                        "invalid_skill_name",
                        Some(path),
                        "skill directory name is not valid UTF-8",
                    ));
                    continue;
                };
                if let Err(message) = validate_skill_name(&name) {
                    recover_or_fail(
                        config,
                        false,
                        &mut diagnostics,
                        "invalid_skill_name",
                        &path,
                        message,
                    )?;
                    continue;
                }

                let skill_path = path.join(SKILL_FILE_NAME);
                let symlink_metadata = match tokio::fs::symlink_metadata(&skill_path).await {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == ErrorKind::NotFound => continue,
                    Err(error) => {
                        recover_or_fail(
                            config,
                            false,
                            &mut diagnostics,
                            "inspect_skill_failed",
                            &skill_path,
                            format!("could not inspect skill file: {error}"),
                        )?;
                        continue;
                    }
                };
                if symlink_metadata.file_type().is_symlink() && !config.follow_symlinks {
                    diagnostics.push(SkillDiagnostic::warning(
                        "symlink_ignored",
                        Some(skill_path),
                        "symbolic-link SKILL.md ignored",
                    ));
                    continue;
                }
                let metadata = match tokio::fs::metadata(&skill_path).await {
                    Ok(metadata) if metadata.is_file() => metadata,
                    Ok(_) => continue,
                    Err(error) => {
                        recover_or_fail(
                            config,
                            false,
                            &mut diagnostics,
                            "inspect_skill_failed",
                            &skill_path,
                            format!("could not inspect skill file: {error}"),
                        )?;
                        continue;
                    }
                };
                if metadata.len() > config.max_skill_bytes as u64 {
                    recover_or_fail(
                        config,
                        false,
                        &mut diagnostics,
                        "skill_too_large",
                        &skill_path,
                        format!(
                            "skill file is {} bytes; configured limit is {} bytes",
                            metadata.len(),
                            config.max_skill_bytes
                        ),
                    )?;
                    continue;
                }
                let canonical_skill_path = match tokio::fs::canonicalize(&skill_path).await {
                    Ok(path) => path,
                    Err(error) => {
                        recover_or_fail(
                            config,
                            false,
                            &mut diagnostics,
                            "canonicalize_skill_failed",
                            &skill_path,
                            format!("could not resolve skill file: {error}"),
                        )?;
                        continue;
                    }
                };
                if !canonical_skill_path.starts_with(&root) {
                    recover_or_fail(
                        config,
                        false,
                        &mut diagnostics,
                        "skill_outside_root",
                        &skill_path,
                        "resolved skill file is outside its configured root",
                    )?;
                    continue;
                }

                loaded_files += 1;
                if loaded_files > config.max_skills {
                    return Err(SkillError::LimitExceeded {
                        maximum: config.max_skills,
                    });
                }
                let bytes = match tokio::fs::read(&canonical_skill_path).await {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        recover_or_fail(
                            config,
                            false,
                            &mut diagnostics,
                            "read_skill_failed",
                            &skill_path,
                            format!("could not read skill file: {error}"),
                        )?;
                        continue;
                    }
                };
                if bytes.len() > config.max_skill_bytes {
                    recover_or_fail(
                        config,
                        false,
                        &mut diagnostics,
                        "skill_too_large",
                        &skill_path,
                        format!(
                            "skill file grew past the configured {} byte limit while reading",
                            config.max_skill_bytes
                        ),
                    )?;
                    continue;
                }
                let raw = match String::from_utf8(bytes) {
                    Ok(raw) => raw,
                    Err(error) => {
                        recover_or_fail(
                            config,
                            false,
                            &mut diagnostics,
                            "invalid_utf8",
                            &skill_path,
                            format!("skill file is not valid UTF-8: {error}"),
                        )?;
                        continue;
                    }
                };
                let loaded = match parse_skill(
                    &name,
                    &canonical_skill_path,
                    directory,
                    &raw,
                    &mut diagnostics,
                ) {
                    Ok(loaded) => Arc::new(loaded),
                    Err(message) => {
                        recover_or_fail(
                            config,
                            false,
                            &mut diagnostics,
                            "invalid_skill",
                            &skill_path,
                            message,
                        )?;
                        continue;
                    }
                };

                if let Some(existing) = by_name.get(&name) {
                    let message = format!(
                        "skill {name:?} from {} conflicts with {}",
                        loaded.metadata.path.display(),
                        existing.metadata.path.display()
                    );
                    match config.duplicate_policy {
                        DuplicateSkillPolicy::Error => {
                            return Err(SkillError::Duplicate {
                                name,
                                first: existing.metadata.path.clone(),
                                second: loaded.metadata.path.clone(),
                            });
                        }
                        DuplicateSkillPolicy::FirstWins => {
                            diagnostics.push(SkillDiagnostic::warning(
                                "duplicate_skill_ignored",
                                Some(loaded.metadata.path.clone()),
                                message,
                            ));
                            continue;
                        }
                        DuplicateSkillPolicy::LastWins => {
                            diagnostics.push(SkillDiagnostic::warning(
                                "duplicate_skill_overridden",
                                Some(existing.metadata.path.clone()),
                                message,
                            ));
                        }
                    }
                }
                by_name.insert(name, loaded);
            }
        }

        let metadata = by_name
            .values()
            .map(|skill| skill.metadata.clone())
            .collect::<Vec<_>>();
        let model_listing = format_model_listing(&metadata, config.listing_char_budget);
        Ok(Self {
            inner: Arc::new(CatalogInner {
                by_name,
                metadata,
                diagnostics,
                model_listing,
                listing_char_budget: config.listing_char_budget,
            }),
        })
    }

    /// Selects an immutable subset of this catalog using exact skill names.
    ///
    /// An explicit allow-list is validated so a misspelled profile entry
    /// cannot silently remove an intended capability. Deny entries take
    /// precedence, and unknown deny entries are harmless.
    pub fn select(&self, allow: Option<&[String]>, deny: &[String]) -> Result<Self, SkillError> {
        if let Some(allow) = allow {
            for name in allow {
                if !self.inner.by_name.contains_key(name) {
                    return Err(SkillError::NotFound { name: name.clone() });
                }
            }
        }

        let denied = deny.iter().map(String::as_str).collect::<HashSet<_>>();
        let selected = |name: &str| {
            !denied.contains(name)
                && allow.is_none_or(|allow| allow.iter().any(|allowed| allowed == name))
        };
        let by_name = self
            .inner
            .by_name
            .iter()
            .filter(|(name, _)| selected(name))
            .map(|(name, skill)| (name.clone(), Arc::clone(skill)))
            .collect::<BTreeMap<_, _>>();
        let metadata = self
            .inner
            .metadata
            .iter()
            .filter(|skill| selected(&skill.name))
            .cloned()
            .collect::<Vec<_>>();
        let model_listing = format_model_listing(&metadata, self.inner.listing_char_budget.max(1));
        Ok(Self {
            inner: Arc::new(CatalogInner {
                by_name,
                metadata,
                diagnostics: self.inner.diagnostics.clone(),
                model_listing,
                listing_char_budget: self.inner.listing_char_budget,
            }),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.inner.metadata.is_empty()
    }

    pub fn len(&self) -> usize {
        self.inner.metadata.len()
    }

    pub fn skills(&self) -> &[SkillMetadata] {
        &self.inner.metadata
    }

    pub fn diagnostics(&self) -> &[SkillDiagnostic] {
        &self.inner.diagnostics
    }

    pub fn model_listing(&self) -> &str {
        &self.inner.model_listing
    }

    pub fn has_model_invocable(&self) -> bool {
        self.inner
            .metadata
            .iter()
            .any(|skill| skill.model_invocable)
    }

    pub fn render_for_model(
        &self,
        name: &str,
        arguments: Option<&str>,
    ) -> Result<RenderedSkill, SkillError> {
        let skill = self.find(name)?;
        if !skill.metadata.model_invocable {
            return Err(SkillError::ModelInvocationDisabled {
                name: skill.metadata.name.clone(),
            });
        }
        Ok(render_skill(skill, arguments))
    }

    pub fn render_for_user(
        &self,
        name: &str,
        arguments: Option<&str>,
    ) -> Result<RenderedSkill, SkillError> {
        let skill = self.find(name)?;
        if !skill.metadata.user_invocable {
            return Err(SkillError::UserInvocationDisabled {
                name: skill.metadata.name.clone(),
            });
        }
        Ok(render_skill(skill, arguments))
    }

    /// Deterministically expands an explicitly selected skill into a user
    /// prompt. Text-only prompts become the skill arguments when no explicit
    /// arguments were supplied; multimodal parts are retained after the skill
    /// instructions.
    pub fn apply_to_prompt(
        &self,
        invocation: &SkillInvocation,
        content: Content,
    ) -> Result<Content, SkillError> {
        match content {
            Content::Text(original) => {
                let arguments = invocation
                    .arguments
                    .as_deref()
                    .or_else(|| (!original.is_empty()).then_some(original.as_str()));
                let rendered =
                    explicitly_selected_content(self.render_for_user(&invocation.name, arguments)?);
                if invocation.arguments.is_none() || original.is_empty() {
                    Ok(Content::Text(rendered))
                } else {
                    Ok(Content::Text(format!(
                        "{}\n\nUser request:\n{}",
                        rendered, original
                    )))
                }
            }
            Content::Parts(parts) => {
                let inferred_arguments = if invocation.arguments.is_none() {
                    let text = parts
                        .iter()
                        .filter_map(ContentPart::as_text)
                        .collect::<Vec<_>>()
                        .join("\n");
                    (!text.is_empty()).then_some(text)
                } else {
                    None
                };
                self.apply_to_multimodal_prompt(
                    invocation,
                    Content::Parts(parts),
                    inferred_arguments,
                )
            }
        }
    }

    fn apply_to_multimodal_prompt(
        &self,
        invocation: &SkillInvocation,
        content: Content,
        inferred_arguments: Option<String>,
    ) -> Result<Content, SkillError> {
        let arguments = invocation
            .arguments
            .as_deref()
            .or(inferred_arguments.as_deref());
        let rendered =
            explicitly_selected_content(self.render_for_user(&invocation.name, arguments)?);
        let Content::Parts(parts) = content else {
            return Ok(Content::Text(rendered));
        };
        let mut combined = Vec::with_capacity(parts.len() + 1);
        combined.push(ContentPart::text(format!(
            "{}\n\nThe user's multimodal request follows.",
            rendered
        )));
        combined.extend(parts);
        Ok(Content::Parts(combined))
    }

    fn find(&self, name: &str) -> Result<&LoadedSkill, SkillError> {
        let normalized = name.trim().strip_prefix('/').unwrap_or(name.trim());
        self.inner
            .by_name
            .get(normalized)
            .map(Arc::as_ref)
            .ok_or_else(|| SkillError::NotFound {
                name: normalized.to_owned(),
            })
    }
}

async fn prepare_root(
    directory: &SkillDirectory,
    config: &SkillsConfig,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> Result<Option<PathBuf>, SkillError> {
    let metadata = match tokio::fs::symlink_metadata(&directory.path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound && !directory.required => {
            return Ok(None);
        }
        Err(error) => {
            recover_or_fail(
                config,
                directory.required,
                diagnostics,
                "skill_directory_unavailable",
                &directory.path,
                format!("skill directory is unavailable: {error}"),
            )?;
            return Ok(None);
        }
    };
    if metadata.file_type().is_symlink() && !config.follow_symlinks {
        recover_or_fail(
            config,
            directory.required,
            diagnostics,
            "symlink_ignored",
            &directory.path,
            "symbolic-link skill root ignored",
        )?;
        return Ok(None);
    }
    let resolved_metadata = match tokio::fs::metadata(&directory.path).await {
        Ok(metadata) => metadata,
        Err(error) => {
            recover_or_fail(
                config,
                directory.required,
                diagnostics,
                "skill_directory_unavailable",
                &directory.path,
                format!("skill directory is unavailable: {error}"),
            )?;
            return Ok(None);
        }
    };
    if !resolved_metadata.is_dir() {
        recover_or_fail(
            config,
            directory.required,
            diagnostics,
            "not_a_directory",
            &directory.path,
            "configured skill root is not a directory",
        )?;
        return Ok(None);
    }
    match tokio::fs::canonicalize(&directory.path).await {
        Ok(path) => Ok(Some(path)),
        Err(error) => {
            recover_or_fail(
                config,
                directory.required,
                diagnostics,
                "canonicalize_directory_failed",
                &directory.path,
                format!("could not resolve skill directory: {error}"),
            )?;
            Ok(None)
        }
    }
}

async fn read_sorted_entries(path: &Path) -> Result<Vec<tokio::fs::DirEntry>, std::io::Error> {
    let mut reader = tokio::fs::read_dir(path).await?;
    let mut entries = Vec::new();
    while let Some(entry) = reader.next_entry().await? {
        entries.push(entry);
    }
    entries.sort_by_key(tokio::fs::DirEntry::file_name);
    Ok(entries)
}

fn recover_or_fail(
    config: &SkillsConfig,
    required: bool,
    diagnostics: &mut Vec<SkillDiagnostic>,
    code: &str,
    path: &Path,
    message: impl Into<String>,
) -> Result<(), SkillError> {
    let message = message.into();
    if config.strict || required {
        return Err(SkillError::Load {
            path: path.to_owned(),
            message,
        });
    }
    diagnostics.push(SkillDiagnostic::error(code, Some(path.to_owned()), message));
    Ok(())
}

fn validate_skill_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("skill name must not be empty".to_owned());
    }
    if name.len() > 128 {
        return Err("skill name must not exceed 128 bytes".to_owned());
    }
    if name.starts_with('/') || name.chars().any(char::is_whitespace) {
        return Err("skill name must not start with `/` or contain whitespace".to_owned());
    }
    if name.chars().any(char::is_control) {
        return Err("skill name must not contain control characters".to_owned());
    }
    Ok(())
}

fn parse_skill(
    name: &str,
    path: &Path,
    directory: &SkillDirectory,
    raw: &str,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> Result<LoadedSkill, String> {
    let (frontmatter, body) = split_frontmatter(raw)?;
    let display_name = optional_scalar(&frontmatter, "name")?;
    let description = optional_scalar(&frontmatter, "description")?
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| description_from_markdown(&body));
    let when_to_use = optional_scalar(&frontmatter, "when_to_use")?;
    let argument_hint = optional_scalar(&frontmatter, "argument-hint")?;
    let argument_names = parse_argument_names(frontmatter.get(YamlValue::from("arguments")))?;
    let version = optional_scalar(&frontmatter, "version")?;
    let disable_model_invocation = bool_field(
        frontmatter.get(YamlValue::from("disable-model-invocation")),
        false,
        "disable-model-invocation",
    )?;
    let user_invocable = bool_field(
        frontmatter.get(YamlValue::from("user-invocable")),
        true,
        "user-invocable",
    )?;

    for field in [
        "allowed-tools",
        "context",
        "agent",
        "model",
        "effort",
        "hooks",
        "shell",
        "paths",
    ] {
        if frontmatter.contains_key(YamlValue::from(field)) {
            diagnostics.push(SkillDiagnostic::warning(
                "unsupported_frontmatter",
                Some(path.to_owned()),
                format!("frontmatter field {field:?} is informational and is not enforced"),
            ));
        }
    }

    Ok(LoadedSkill {
        metadata: SkillMetadata {
            name: name.to_owned(),
            display_name,
            description,
            when_to_use,
            argument_hint,
            argument_names,
            version,
            model_invocable: !disable_model_invocation,
            user_invocable,
            source: directory.source.clone(),
            path: path.to_owned(),
        },
        body,
        base_dir: path
            .parent()
            .expect("SKILL.md path always has a parent")
            .to_owned(),
    })
}

fn split_frontmatter(raw: &str) -> Result<(Mapping, String), String> {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let mut lines = raw.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Ok((Mapping::new(), String::new()));
    };
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return Ok((Mapping::new(), raw.to_owned()));
    }

    let yaml_start = first.len();
    let mut offset = yaml_start;
    for line in lines {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            let yaml = &raw[yaml_start..offset];
            let parsed = if yaml.trim().is_empty() {
                YamlValue::Mapping(Mapping::new())
            } else {
                serde_yaml_ng::from_str::<YamlValue>(yaml)
                    .map_err(|error| format!("invalid YAML frontmatter: {error}"))?
            };
            let YamlValue::Mapping(mapping) = parsed else {
                return Err("YAML frontmatter must be a mapping".to_owned());
            };
            return Ok((mapping, raw[offset + line.len()..].to_owned()));
        }
        offset += line.len();
    }
    Err("frontmatter starts with `---` but has no closing delimiter".to_owned())
}

fn optional_scalar(mapping: &Mapping, key: &str) -> Result<Option<String>, String> {
    let Some(value) = mapping.get(YamlValue::from(key)) else {
        return Ok(None);
    };
    match value {
        YamlValue::Null => Ok(None),
        YamlValue::String(value) => Ok(Some(value.clone())),
        YamlValue::Bool(value) => Ok(Some(value.to_string())),
        YamlValue::Number(value) => Ok(Some(value.to_string())),
        _ => Err(format!("frontmatter field {key:?} must be a scalar")),
    }
}

fn bool_field(value: Option<&YamlValue>, default: bool, field: &str) -> Result<bool, String> {
    let Some(value) = value else {
        return Ok(default);
    };
    match value {
        YamlValue::Null => Ok(default),
        YamlValue::Bool(value) => Ok(*value),
        YamlValue::Number(value) if value.as_i64() == Some(0) => Ok(false),
        YamlValue::Number(value) if value.as_i64() == Some(1) => Ok(true),
        YamlValue::String(value) if value.eq_ignore_ascii_case("true") || value == "1" => Ok(true),
        YamlValue::String(value) if value.eq_ignore_ascii_case("false") || value == "0" => {
            Ok(false)
        }
        _ => Err(format!("frontmatter field {field:?} must be a boolean")),
    }
}

fn parse_argument_names(value: Option<&YamlValue>) -> Result<Vec<String>, String> {
    let candidates = match value {
        None | Some(YamlValue::Null) => Vec::new(),
        Some(YamlValue::String(value)) => value
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        Some(YamlValue::Sequence(values)) => values
            .iter()
            .map(|value| match value {
                YamlValue::String(value) => Ok(value.clone()),
                _ => Err("frontmatter `arguments` entries must be strings".to_owned()),
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err("frontmatter `arguments` must be a string or string array".to_owned());
        }
    };
    Ok(candidates
        .into_iter()
        .filter(|name| {
            !name.trim().is_empty() && !name.chars().all(|character| character.is_ascii_digit())
        })
        .collect())
}

fn description_from_markdown(markdown: &str) -> String {
    markdown
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_start_matches('#').trim())
        .filter(|line| !line.is_empty())
        .unwrap_or("No description provided.")
        .to_owned()
}

fn render_skill(skill: &LoadedSkill, arguments: Option<&str>) -> RenderedSkill {
    let base_dir = skill.base_dir.to_string_lossy().replace('\\', "/");
    let initial = format!(
        "Base directory for this skill: {base_dir}\n\n{}",
        skill.body
    );
    let content = substitute_arguments(&initial, arguments, &skill.metadata.argument_names)
        .replace("${PHI_SKILL_DIR}", &base_dir)
        .replace("${PHY_SKILL_DIR}", &base_dir)
        .replace("${CLAUDE_SKILL_DIR}", &base_dir);
    RenderedSkill {
        name: skill.metadata.name.clone(),
        content,
    }
}

fn explicitly_selected_content(rendered: RenderedSkill) -> String {
    format!(
        "The skill {:?} has already been selected and loaded for this turn. Follow it directly; do not call the skill tool for it again.\n\n{}",
        rendered.name, rendered.content
    )
}

fn substitute_arguments(content: &str, arguments: Option<&str>, names: &[String]) -> String {
    let Some(arguments) = arguments else {
        return content.to_owned();
    };
    let parsed = parse_arguments(arguments);
    let mut rendered = String::with_capacity(content.len() + arguments.len());
    let mut cursor = 0usize;
    let mut replaced = false;
    while cursor < content.len() {
        let remaining = &content[cursor..];
        if remaining.starts_with('$')
            && let Some((length, replacement)) = placeholder(remaining, arguments, &parsed, names)
        {
            rendered.push_str(replacement);
            cursor += length;
            replaced = true;
            continue;
        }
        let character = remaining
            .chars()
            .next()
            .expect("cursor is within the string");
        rendered.push(character);
        cursor += character.len_utf8();
    }
    if !replaced && !arguments.is_empty() {
        rendered.push_str("\n\nARGUMENTS: ");
        rendered.push_str(arguments);
    }
    rendered
}

fn placeholder<'a>(
    remaining: &str,
    all: &'a str,
    parsed: &'a [String],
    names: &[String],
) -> Option<(usize, &'a str)> {
    if let Some(after) = remaining.strip_prefix("$ARGUMENTS[")
        && let Some(close) = after.find(']')
        && !after[..close].is_empty()
        && after[..close].bytes().all(|byte| byte.is_ascii_digit())
    {
        let index = after[..close].parse::<usize>().ok()?;
        return Some(("$ARGUMENTS[".len() + close + 1, value_at(parsed, index)));
    }
    if let Some(after) = remaining.strip_prefix("$ARGUMENTS")
        && after
            .as_bytes()
            .first()
            .is_none_or(|byte| !is_word(*byte) && *byte != b'[')
    {
        return Some(("$ARGUMENTS".len(), all));
    }
    if let Some(after) = remaining.strip_prefix('$') {
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0
            && after
                .as_bytes()
                .get(digits)
                .is_none_or(|byte| !is_word(*byte))
        {
            let index = after[..digits].parse::<usize>().ok()?;
            return Some((digits + 1, value_at(parsed, index)));
        }
    }
    for (index, name) in names.iter().enumerate() {
        let token = format!("${name}");
        if let Some(after) = remaining.strip_prefix(&token)
            && after
                .as_bytes()
                .first()
                .is_none_or(|byte| !is_word(*byte) && *byte != b'[')
        {
            return Some((token.len(), value_at(parsed, index)));
        }
    }
    None
}

fn value_at(values: &[String], index: usize) -> &str {
    values.get(index).map(String::as_str).unwrap_or("")
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn parse_arguments(arguments: &str) -> Vec<String> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut values = Vec::new();
    let mut current = String::new();
    let mut quote = Quote::None;
    let mut escaped = false;
    let mut started = false;
    for character in arguments.chars() {
        if escaped {
            current.push(character);
            started = true;
            escaped = false;
            continue;
        }
        match (quote, character) {
            (Quote::None, '\\') | (Quote::Double, '\\') => escaped = true,
            (Quote::None, '\'') => {
                quote = Quote::Single;
                started = true;
            }
            (Quote::Single, '\'') => quote = Quote::None,
            (Quote::None, '"') => {
                quote = Quote::Double;
                started = true;
            }
            (Quote::Double, '"') => quote = Quote::None,
            (Quote::None, character) if character.is_whitespace() => {
                if started {
                    values.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (_, character) => {
                current.push(character);
                started = true;
            }
        }
    }
    if escaped {
        current.push('\\');
    }
    if quote != Quote::None {
        return arguments.split_whitespace().map(str::to_owned).collect();
    }
    if started {
        values.push(current);
    }
    values
}

fn format_model_listing(metadata: &[SkillMetadata], budget: usize) -> String {
    let skills = metadata
        .iter()
        .filter(|skill| skill.model_invocable)
        .collect::<Vec<_>>();
    if skills.is_empty() {
        return String::new();
    }
    let descriptions = skills
        .iter()
        .map(|skill| {
            let description = match &skill.when_to_use {
                Some(when) if !when.trim().is_empty() => {
                    format!("{} - {when}", skill.description)
                }
                _ => skill.description.clone(),
            };
            truncate_chars(&description, MAX_LISTING_DESCRIPTION_CHARS)
        })
        .collect::<Vec<_>>();
    let full = skills
        .iter()
        .zip(&descriptions)
        .map(|(skill, description)| format!("- {}: {description}", skill.name))
        .collect::<Vec<_>>()
        .join("\n");
    if full.chars().count() <= budget {
        return full;
    }

    let name_overhead = skills
        .iter()
        .map(|skill| skill.name.chars().count() + 4)
        .sum::<usize>()
        .saturating_add(skills.len().saturating_sub(1));
    let available = budget.saturating_sub(name_overhead) / skills.len();
    if available < MIN_LISTING_DESCRIPTION_CHARS {
        return skills
            .iter()
            .map(|skill| format!("- {}", skill.name))
            .collect::<Vec<_>>()
            .join("\n");
    }
    skills
        .iter()
        .zip(descriptions)
        .map(|(skill, description)| {
            format!(
                "- {}: {}",
                skill.name,
                truncate_chars(&description, available)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        return value.to_owned();
    }
    if maximum == 0 {
        return String::new();
    }
    value
        .chars()
        .take(maximum.saturating_sub(1))
        .chain(std::iter::once('…'))
        .collect()
}

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("could not load skill at {path}: {message}")]
    Load { path: PathBuf, message: String },

    #[error("duplicate skill {name:?}: {first} and {second}")]
    Duplicate {
        name: String,
        first: PathBuf,
        second: PathBuf,
    },

    #[error("skill discovery exceeded the configured limit of {maximum} files")]
    LimitExceeded { maximum: usize },

    #[error("skill {name:?} was not found in this session")]
    NotFound { name: String },

    #[error("skill {name:?} cannot be invoked by the model")]
    ModelInvocationDisabled { name: String },

    #[error("skill {name:?} cannot be invoked explicitly")]
    UserInvocationDisabled { name: String },
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn write_skill(root: &Path, name: &str, contents: &str) {
        let directory = root.join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(SKILL_FILE_NAME), contents).unwrap();
    }

    #[tokio::test]
    async fn discovers_multiple_roots_and_later_root_wins() {
        let global = TempDir::new().unwrap();
        let workspace = TempDir::new().unwrap();
        write_skill(
            global.path(),
            "review",
            "---\ndescription: global\n---\nglobal body",
        );
        write_skill(
            workspace.path(),
            "review",
            "---\ndescription: workspace\n---\nworkspace $ARGUMENTS",
        );
        write_skill(workspace.path(), "test", "# Run tests\n\nDo it.");

        let config = SkillsConfig::new()
            .skill_directory(SkillDirectory::new(global.path()).source("global"))
            .skill_directory(SkillDirectory::new(workspace.path()).source("workspace"));
        let catalog = SkillCatalog::load(&config).await.unwrap();

        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog.skills()[0].name, "review");
        assert_eq!(catalog.skills()[0].description, "workspace");
        assert_eq!(catalog.skills()[0].source.as_deref(), Some("workspace"));
        assert!(
            catalog
                .render_for_user("/review", Some("security"))
                .unwrap()
                .content
                .contains("workspace security")
        );
        assert!(
            catalog
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == "duplicate_skill_overridden")
        );
    }

    #[tokio::test]
    async fn honors_invocation_flags_and_argument_forms() {
        let root = TempDir::new().unwrap();
        write_skill(
            root.path(),
            "deploy",
            "---\ndescription: deploy\narguments: [env, tag]\ndisable-model-invocation: true\n---\n$env $tag $ARGUMENTS[1] $0 ${CLAUDE_SKILL_DIR}",
        );
        let catalog = SkillCatalog::load(&SkillsConfig::new().directory(root.path()))
            .await
            .unwrap();

        assert!(matches!(
            catalog.render_for_model("deploy", None),
            Err(SkillError::ModelInvocationDisabled { .. })
        ));
        let rendered = catalog
            .render_for_user("deploy", Some("prod \"v 1\""))
            .unwrap();
        assert!(rendered.content.contains("prod v 1 v 1 prod"));
        assert!(
            rendered
                .content
                .contains(&root.path().join("deploy").display().to_string())
        );
    }

    #[tokio::test]
    async fn explicit_text_prompt_becomes_arguments() {
        let root = TempDir::new().unwrap();
        write_skill(root.path(), "explain", "Explain: $ARGUMENTS");
        let catalog = SkillCatalog::load(&SkillsConfig::new().directory(root.path()))
            .await
            .unwrap();

        let content = catalog
            .apply_to_prompt(&SkillInvocation::new("explain"), Content::text("ownership"))
            .unwrap();
        assert!(content.as_text().unwrap().contains("Explain: ownership"));
    }

    #[tokio::test]
    async fn profile_selection_is_exact_deny_first_and_rejects_unknown_allows() {
        let root = TempDir::new().unwrap();
        write_skill(root.path(), "review", "Review the change.");
        write_skill(root.path(), "test", "Run the tests.");
        let catalog = SkillCatalog::load(&SkillsConfig::new().directory(root.path()))
            .await
            .unwrap();

        let selected = catalog
            .select(
                Some(&["review".to_owned(), "test".to_owned()]),
                &["test".to_owned(), "unknown-deny".to_owned()],
            )
            .unwrap();
        assert_eq!(
            selected
                .skills()
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            ["review"]
        );
        assert!(selected.model_listing().contains("review"));
        assert!(!selected.model_listing().contains("test"));

        assert!(matches!(
            catalog.select(Some(&["missing".to_owned()]), &[]),
            Err(SkillError::NotFound { name }) if name == "missing"
        ));
    }

    #[tokio::test]
    async fn disabled_config_does_not_scan_required_directories() {
        let config = SkillsConfig::disabled()
            .skill_directory(SkillDirectory::new("/definitely/missing").required(true));
        let catalog = SkillCatalog::load(&config).await.unwrap();
        assert!(catalog.is_empty());
    }
}
