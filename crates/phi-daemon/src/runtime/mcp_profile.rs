use std::{collections::BTreeMap, fmt};

use reqwest::{
    Url,
    header::{HeaderName, HeaderValue},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_MCP_PROFILE_ID_BYTES: usize = 64;
pub const MAX_MCP_TOOL_PREFIX_BYTES: usize = 64;
pub const MAX_MCP_COMMAND_BYTES: usize = 4 * 1024;
pub const MAX_MCP_URL_BYTES: usize = 16 * 1024;
pub const MAX_MCP_ARGUMENTS: usize = 256;
pub const MAX_MCP_ARGUMENT_BYTES: usize = 16 * 1024;
pub const MAX_MCP_ENVIRONMENT_ENTRIES: usize = 256;
pub const MAX_MCP_HEADERS: usize = 256;
pub const MAX_MCP_SECRET_BYTES: usize = 64 * 1024;
pub const MAX_MCP_OUTPUT_LINES: usize = 100_000;
pub const MAX_MCP_OUTPUT_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_MCP_TIMEOUT_SECS: u64 = 24 * 60 * 60;
pub const DEFAULT_MCP_CONNECT_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_MCP_REQUEST_TIMEOUT_SECS: u64 = 60;
pub const DEFAULT_MCP_OUTPUT_LINES: usize = phi::DEFAULT_MAX_LINES;
pub const DEFAULT_MCP_OUTPUT_BYTES: usize = phi::DEFAULT_MAX_BYTES;

fn default_connect_timeout_secs() -> u64 {
    DEFAULT_MCP_CONNECT_TIMEOUT_SECS
}

fn default_request_timeout_secs() -> Option<u64> {
    Some(DEFAULT_MCP_REQUEST_TIMEOUT_SECS)
}

fn default_output_lines() -> usize {
    DEFAULT_MCP_OUTPUT_LINES
}

fn default_output_bytes() -> usize {
    DEFAULT_MCP_OUTPUT_BYTES
}

fn default_true() -> bool {
    true
}

/// Secret-bearing daemon configuration for one MCP connection.
///
/// This type is persisted only in the owner-readable MCP profile store. Public
/// HTTP responses use a separate redacted DTO.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpProfileDefinition {
    pub transport: McpTransportDefinition,
    pub tool_name_prefix: String,
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: Option<u64>,
    #[serde(default = "default_output_lines")]
    pub max_output_lines: usize,
    #[serde(default = "default_output_bytes")]
    pub max_output_bytes: usize,
}

impl fmt::Debug for McpProfileDefinition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpProfileDefinition")
            .field("transport", &self.transport)
            .field("tool_name_prefix", &self.tool_name_prefix)
            .field("connect_timeout_secs", &self.connect_timeout_secs)
            .field("request_timeout_secs", &self.request_timeout_secs)
            .field("max_output_lines", &self.max_output_lines)
            .field("max_output_bytes", &self.max_output_bytes)
            .finish()
    }
}

impl McpProfileDefinition {
    pub fn normalized(&self) -> Result<Self, McpProfileValidationError> {
        if self.connect_timeout_secs == 0 || self.connect_timeout_secs > MAX_MCP_TIMEOUT_SECS {
            return Err(invalid(
                "connect_timeout_secs",
                format!("must be between 1 and {MAX_MCP_TIMEOUT_SECS}"),
            ));
        }
        if let Some(timeout) = self.request_timeout_secs
            && (timeout == 0 || timeout > MAX_MCP_TIMEOUT_SECS)
        {
            return Err(invalid(
                "request_timeout_secs",
                format!("must be null or between 1 and {MAX_MCP_TIMEOUT_SECS}"),
            ));
        }
        if self.max_output_lines == 0 || self.max_output_lines > MAX_MCP_OUTPUT_LINES {
            return Err(invalid(
                "max_output_lines",
                format!("must be between 1 and {MAX_MCP_OUTPUT_LINES}"),
            ));
        }
        if self.max_output_bytes == 0 || self.max_output_bytes > MAX_MCP_OUTPUT_BYTES {
            return Err(invalid(
                "max_output_bytes",
                format!("must be between 1 and {MAX_MCP_OUTPUT_BYTES}"),
            ));
        }

        Ok(Self {
            transport: self.transport.normalized()?,
            tool_name_prefix: normalize_tool_prefix(&self.tool_name_prefix)?,
            connect_timeout_secs: self.connect_timeout_secs,
            request_timeout_secs: self.request_timeout_secs,
            max_output_lines: self.max_output_lines,
            max_output_bytes: self.max_output_bytes,
        })
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum McpTransportDefinition {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        current_dir: Option<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default)]
        clear_env: bool,
    },
    Http {
        url: String,
        #[serde(default)]
        bearer_token: Option<String>,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default = "default_true")]
        allow_stateless: bool,
        #[serde(default = "default_true")]
        reinitialize_on_expired_session: bool,
    },
}

impl fmt::Debug for McpTransportDefinition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stdio {
                command,
                args,
                current_dir,
                env,
                clear_env,
            } => formatter
                .debug_struct("Stdio")
                .field("command", command)
                .field("args", args)
                .field("current_dir", current_dir)
                .field("env_keys", &env.keys().collect::<Vec<_>>())
                .field("clear_env", clear_env)
                .finish(),
            Self::Http {
                url,
                bearer_token,
                headers,
                allow_stateless,
                reinitialize_on_expired_session,
            } => formatter
                .debug_struct("Http")
                .field("url", url)
                .field("bearer_token", &bearer_token.as_ref().map(|_| "[REDACTED]"))
                .field("header_names", &headers.keys().collect::<Vec<_>>())
                .field("allow_stateless", allow_stateless)
                .field(
                    "reinitialize_on_expired_session",
                    reinitialize_on_expired_session,
                )
                .finish(),
        }
    }
}

impl McpTransportDefinition {
    fn normalized(&self) -> Result<Self, McpProfileValidationError> {
        match self {
            Self::Stdio {
                command,
                args,
                current_dir,
                env,
                clear_env,
            } => {
                let command =
                    normalize_required("transport.command", command, MAX_MCP_COMMAND_BYTES)?;
                if args.len() > MAX_MCP_ARGUMENTS {
                    return Err(invalid(
                        "transport.args",
                        format!("must not contain more than {MAX_MCP_ARGUMENTS} entries"),
                    ));
                }
                let args = args
                    .iter()
                    .enumerate()
                    .map(|(index, argument)| {
                        validate_untrimmed(
                            "transport.args",
                            argument,
                            MAX_MCP_ARGUMENT_BYTES,
                            &format!("argument {index}"),
                        )?;
                        Ok(argument.clone())
                    })
                    .collect::<Result<Vec<_>, McpProfileValidationError>>()?;
                let current_dir = current_dir
                    .as_deref()
                    .map(|path| {
                        normalize_required("transport.current_dir", path, MAX_MCP_URL_BYTES)
                    })
                    .transpose()?;
                if env.len() > MAX_MCP_ENVIRONMENT_ENTRIES {
                    return Err(invalid(
                        "transport.env",
                        format!("must not contain more than {MAX_MCP_ENVIRONMENT_ENTRIES} entries"),
                    ));
                }
                let mut normalized_env = BTreeMap::new();
                for (name, value) in env {
                    validate_env_name(name)?;
                    validate_secret("transport.env", value)?;
                    normalized_env.insert(name.clone(), value.clone());
                }
                Ok(Self::Stdio {
                    command,
                    args,
                    current_dir,
                    env: normalized_env,
                    clear_env: *clear_env,
                })
            }
            Self::Http {
                url,
                bearer_token,
                headers,
                allow_stateless,
                reinitialize_on_expired_session,
            } => {
                let url = normalize_required("transport.url", url, MAX_MCP_URL_BYTES)?;
                let parsed = Url::parse(&url)
                    .map_err(|_| invalid("transport.url", "must be a valid HTTP(S) URL"))?;
                if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                    return Err(invalid(
                        "transport.url",
                        "must be an absolute HTTP(S) URL with a host",
                    ));
                }
                if !parsed.username().is_empty() || parsed.password().is_some() {
                    return Err(invalid(
                        "transport.url",
                        "must not contain embedded credentials",
                    ));
                }
                if parsed.fragment().is_some() {
                    return Err(invalid("transport.url", "must not contain a fragment"));
                }
                let bearer_token = bearer_token
                    .as_deref()
                    .map(|token| {
                        let token = token.trim();
                        if token.is_empty() {
                            return Err(invalid(
                                "transport.bearer_token",
                                "must not be empty when provided",
                            ));
                        }
                        validate_secret("transport.bearer_token", token)?;
                        Ok(token.to_owned())
                    })
                    .transpose()?;
                if headers.len() > MAX_MCP_HEADERS {
                    return Err(invalid(
                        "transport.headers",
                        format!("must not contain more than {MAX_MCP_HEADERS} entries"),
                    ));
                }
                let mut normalized_headers = BTreeMap::new();
                for (name, value) in headers {
                    let parsed_name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                        invalid("transport.headers", format!("invalid header name {name:?}"))
                    })?;
                    HeaderValue::from_str(value).map_err(|_| {
                        invalid(
                            "transport.headers",
                            format!("header {name:?} has an invalid value"),
                        )
                    })?;
                    validate_secret("transport.headers", value)?;
                    let normalized_name = parsed_name.as_str().to_owned();
                    if normalized_headers
                        .insert(normalized_name.clone(), value.clone())
                        .is_some()
                    {
                        return Err(invalid(
                            "transport.headers",
                            format!("duplicate header name {normalized_name:?}"),
                        ));
                    }
                }
                Ok(Self::Http {
                    url,
                    bearer_token,
                    headers: normalized_headers,
                    allow_stateless: *allow_stateless,
                    reinitialize_on_expired_session: *reinitialize_on_expired_session,
                })
            }
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpProfile {
    pub mcp_profile_id: String,
    pub revision: u64,
    pub definition: McpProfileDefinition,
}

impl fmt::Debug for McpProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpProfile")
            .field("mcp_profile_id", &self.mcp_profile_id)
            .field("revision", &self.revision)
            .field("definition", &self.definition)
            .finish()
    }
}

impl McpProfile {
    pub fn normalized(&self) -> Result<Self, McpProfileValidationError> {
        validate_mcp_profile_id(&self.mcp_profile_id)?;
        if self.revision == 0 {
            return Err(invalid("revision", "must be greater than zero"));
        }
        Ok(Self {
            mcp_profile_id: self.mcp_profile_id.clone(),
            revision: self.revision,
            definition: self.definition.normalized()?,
        })
    }
}

pub fn validate_mcp_profile_id(mcp_profile_id: &str) -> Result<(), McpProfileValidationError> {
    validate_identifier("mcp_profile_id", mcp_profile_id, MAX_MCP_PROFILE_ID_BYTES)
}

fn normalize_tool_prefix(prefix: &str) -> Result<String, McpProfileValidationError> {
    validate_identifier("tool_name_prefix", prefix, MAX_MCP_TOOL_PREFIX_BYTES)?;
    Ok(prefix.to_owned())
}

fn validate_identifier(
    field: &'static str,
    value: &str,
    maximum: usize,
) -> Result<(), McpProfileValidationError> {
    if value.is_empty() {
        return Err(invalid(field, "must not be empty"));
    }
    if value != value.trim() {
        return Err(invalid(field, "must not have surrounding whitespace"));
    }
    if value.len() > maximum {
        return Err(invalid(field, format!("must not exceed {maximum} bytes")));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(invalid(
            field,
            "must contain only ASCII letters, digits, '_' or '-'",
        ));
    }
    Ok(())
}

fn normalize_required(
    field: &'static str,
    value: &str,
    maximum: usize,
) -> Result<String, McpProfileValidationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid(field, "must not be empty"));
    }
    validate_untrimmed(field, value, maximum, "value")?;
    Ok(value.to_owned())
}

fn validate_untrimmed(
    field: &'static str,
    value: &str,
    maximum: usize,
    label: &str,
) -> Result<(), McpProfileValidationError> {
    if value.len() > maximum {
        return Err(invalid(
            field,
            format!("{label} must not exceed {maximum} bytes"),
        ));
    }
    if value.contains('\0') {
        return Err(invalid(field, format!("{label} must not contain NUL")));
    }
    Ok(())
}

fn validate_env_name(name: &str) -> Result<(), McpProfileValidationError> {
    if name.is_empty() {
        return Err(invalid(
            "transport.env",
            "environment name must not be empty",
        ));
    }
    if name.len() > 256 {
        return Err(invalid(
            "transport.env",
            "environment name must not exceed 256 bytes",
        ));
    }
    if name.contains(['=', '\0']) {
        return Err(invalid(
            "transport.env",
            format!("invalid environment name {name:?}"),
        ));
    }
    Ok(())
}

fn validate_secret(field: &'static str, value: &str) -> Result<(), McpProfileValidationError> {
    if value.len() > MAX_MCP_SECRET_BYTES {
        return Err(invalid(
            field,
            format!("secret value must not exceed {MAX_MCP_SECRET_BYTES} bytes"),
        ));
    }
    if value.contains('\0') {
        return Err(invalid(field, "secret value must not contain NUL"));
    }
    Ok(())
}

fn invalid(field: &'static str, message: impl Into<String>) -> McpProfileValidationError {
    McpProfileValidationError::InvalidField {
        field,
        message: message.into(),
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum McpProfileValidationError {
    #[error("invalid MCP Profile field {field}: {message}")]
    InvalidField {
        field: &'static str,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio_definition() -> McpProfileDefinition {
        McpProfileDefinition {
            transport: McpTransportDefinition::Stdio {
                command: " npx ".to_owned(),
                args: vec!["-y".to_owned(), "@example/mcp".to_owned()],
                current_dir: Some(" tools ".to_owned()),
                env: BTreeMap::from([("TOKEN".to_owned(), "secret".to_owned())]),
                clear_env: false,
            },
            tool_name_prefix: "example".to_owned(),
            connect_timeout_secs: DEFAULT_MCP_CONNECT_TIMEOUT_SECS,
            request_timeout_secs: Some(DEFAULT_MCP_REQUEST_TIMEOUT_SECS),
            max_output_lines: DEFAULT_MCP_OUTPUT_LINES,
            max_output_bytes: DEFAULT_MCP_OUTPUT_BYTES,
        }
    }

    #[test]
    fn normalizes_stdio_without_exposing_environment_values_in_debug() {
        let normalized = stdio_definition().normalized().unwrap();
        let McpTransportDefinition::Stdio {
            command,
            current_dir,
            ..
        } = &normalized.transport
        else {
            panic!("expected stdio");
        };
        assert_eq!(command, "npx");
        assert_eq!(current_dir.as_deref(), Some("tools"));
        let debug = format!("{normalized:?}");
        assert!(debug.contains("TOKEN"));
        assert!(!debug.contains("secret"));
    }

    #[test]
    fn normalizes_http_and_redacts_credentials() {
        let definition = McpProfileDefinition {
            transport: McpTransportDefinition::Http {
                url: " https://mcp.example.test/api ".to_owned(),
                bearer_token: Some(" bearer-credential-123 ".to_owned()),
                headers: BTreeMap::from([(
                    "X-Secret".to_owned(),
                    "header-credential-456".to_owned(),
                )]),
                allow_stateless: true,
                reinitialize_on_expired_session: true,
            },
            tool_name_prefix: "remote".to_owned(),
            connect_timeout_secs: 30,
            request_timeout_secs: None,
            max_output_lines: 100,
            max_output_bytes: 4096,
        }
        .normalized()
        .unwrap();
        let debug = format!("{definition:?}");
        assert!(debug.contains("x-secret"));
        assert!(!debug.contains("bearer-credential-123"));
        assert!(!debug.contains("header-credential-456"));
    }

    #[test]
    fn rejects_unsafe_identifiers_urls_and_timeouts() {
        assert!(validate_mcp_profile_id("bad profile").is_err());
        let mut definition = stdio_definition();
        definition.connect_timeout_secs = 0;
        assert!(definition.normalized().is_err());

        definition = stdio_definition();
        definition.transport = McpTransportDefinition::Http {
            url: "https://user:password@example.test/mcp".to_owned(),
            bearer_token: None,
            headers: BTreeMap::new(),
            allow_stateless: true,
            reinitialize_on_expired_session: true,
        };
        assert!(definition.normalized().is_err());
    }
}
