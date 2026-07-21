use std::{
    env, fmt, fs,
    io::{self, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

use thiserror::Error;

use phi::{DuplicateSkillPolicy, SkillDirectory, SkillsConfig};

pub const BIND_ADDRESS_ENV: &str = "PHI_DAEMON_BIND";
pub const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:8787";
pub const PUBLIC_URL_ENV: &str = "PHI_DAEMON_PUBLIC_URL";
pub const DATA_DIR_ENV: &str = "PHI_DAEMON_DATA_DIR";
pub const DEFAULT_DATA_DIR: &str = ".phi/daemon";
pub const AUTH_KEY_FILE_ENV: &str = "PHI_DAEMON_AUTH_KEY_FILE";
pub const DEFAULT_AUTH_KEY_FILE: &str = ".phi/daemon/auth.key";
pub const TLS_CERT_FILE_ENV: &str = "PHI_DAEMON_TLS_CERT_FILE";
pub const TLS_KEY_FILE_ENV: &str = "PHI_DAEMON_TLS_KEY_FILE";
pub const SKILLS_ENABLED_ENV: &str = "PHI_DAEMON_SKILLS_ENABLED";
pub const SUBAGENTS_ENABLED_ENV: &str = "PHI_DAEMON_SUBAGENTS_ENABLED";
pub const SESSION_TITLE_PROFILE_ID_ENV: &str = "PHI_DAEMON_SESSION_TITLE_PROFILE_ID";
pub const WORKSPACE_DIR_ENV: &str = "PHI_DAEMON_WORKSPACE_DIR";
pub const GLOBAL_SKILLS_DIRS_ENV: &str = "PHI_DAEMON_GLOBAL_SKILLS_DIRS";
pub const WORKSPACE_SKILLS_DIRS_ENV: &str = "PHI_DAEMON_WORKSPACE_SKILLS_DIRS";
pub const HTTP_PROXY_ENV: &str = "HTTP_PROXY";
pub const HTTPS_PROXY_ENV: &str = "HTTPS_PROXY";
pub const ALL_PROXY_ENV: &str = "ALL_PROXY";
pub const NO_PROXY_ENV: &str = "NO_PROXY";
pub const DEFAULT_GLOBAL_SKILLS_DIR: &str = ".phy/skills";
pub const DEFAULT_WORKSPACE_SKILLS_DIRS: [&str; 2] = [".phy/skills", ".claude/skills"];

const MIN_AUTH_KEY_BYTES: usize = 32;
const MAX_AUTH_KEY_BYTES: usize = 4096;
const GENERATED_AUTH_KEY_BYTES: usize = 32;

#[derive(Clone, PartialEq, Eq)]
pub struct TlsConfig {
    certificate_file: PathBuf,
    private_key_file: PathBuf,
}

impl TlsConfig {
    pub fn new(
        certificate_file: impl Into<PathBuf>,
        private_key_file: impl Into<PathBuf>,
    ) -> Result<Self, ConfigError> {
        Ok(Self {
            certificate_file: validate_tls_file_path(TLS_CERT_FILE_ENV, certificate_file.into())?,
            private_key_file: validate_tls_file_path(TLS_KEY_FILE_ENV, private_key_file.into())?,
        })
    }

    pub fn certificate_file(&self) -> &Path {
        &self.certificate_file
    }

    pub fn private_key_file(&self) -> &Path {
        &self.private_key_file
    }
}

impl fmt::Debug for TlsConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TlsConfig")
            .field("certificate_file", &self.certificate_file)
            .field("private_key_file", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
struct OutboundProxyConfig {
    http_proxy: Option<String>,
    https_proxy: Option<String>,
    all_proxy: Option<String>,
    no_proxy: Option<String>,
}

impl OutboundProxyConfig {
    fn from_env() -> Result<Self, ConfigError> {
        Self::from_values(
            optional_environment_with_alias(HTTP_PROXY_ENV, "http_proxy")?,
            optional_environment_with_alias(HTTPS_PROXY_ENV, "https_proxy")?,
            optional_environment_with_alias(ALL_PROXY_ENV, "all_proxy")?,
            optional_environment_with_alias(NO_PROXY_ENV, "no_proxy")?,
        )
    }

    fn from_values(
        http_proxy: Option<String>,
        https_proxy: Option<String>,
        all_proxy: Option<String>,
        no_proxy: Option<String>,
    ) -> Result<Self, ConfigError> {
        Ok(Self {
            http_proxy: normalize_proxy_url(HTTP_PROXY_ENV, http_proxy)?,
            https_proxy: normalize_proxy_url(HTTPS_PROXY_ENV, https_proxy)?,
            all_proxy: normalize_proxy_url(ALL_PROXY_ENV, all_proxy)?,
            no_proxy: normalize_optional_environment(no_proxy),
        })
    }

    fn http_client(&self) -> Result<reqwest::Client, ConfigError> {
        let no_proxy = self
            .no_proxy
            .as_deref()
            .and_then(reqwest::NoProxy::from_string);
        let mut builder = reqwest::Client::builder().no_proxy();

        if let Some(url) = &self.http_proxy {
            let proxy = reqwest::Proxy::http(url)
                .map_err(|_| ConfigError::InvalidProxyUrl {
                    name: HTTP_PROXY_ENV,
                })?
                .no_proxy(no_proxy.clone());
            builder = builder.proxy(proxy);
        }
        if let Some(url) = &self.https_proxy {
            let proxy = reqwest::Proxy::https(url)
                .map_err(|_| ConfigError::InvalidProxyUrl {
                    name: HTTPS_PROXY_ENV,
                })?
                .no_proxy(no_proxy.clone());
            builder = builder.proxy(proxy);
        }
        if let Some(url) = &self.all_proxy {
            let proxy = reqwest::Proxy::all(url)
                .map_err(|_| ConfigError::InvalidProxyUrl {
                    name: ALL_PROXY_ENV,
                })?
                .no_proxy(no_proxy);
            builder = builder.proxy(proxy);
        }

        builder
            .build()
            .map_err(|_| ConfigError::ProviderHttpClientInitialization)
    }
}

impl fmt::Debug for OutboundProxyConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OutboundProxyConfig")
            .field("http_proxy_configured", &self.http_proxy.is_some())
            .field("https_proxy_configured", &self.https_proxy.is_some())
            .field("all_proxy_configured", &self.all_proxy.is_some())
            .field("no_proxy_configured", &self.no_proxy.is_some())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DaemonConfig {
    bind_address: SocketAddr,
    public_url: Option<String>,
    data_dir: PathBuf,
    auth_key: String,
    tls: Option<TlsConfig>,
    qr_enabled: bool,
    skills_enabled: bool,
    subagents_enabled: bool,
    session_title_profile_id: Option<String>,
    workspace_dir: PathBuf,
    global_skill_dirs: Vec<PathBuf>,
    workspace_skill_dirs: Vec<PathBuf>,
    outbound_proxy: OutboundProxyConfig,
}

impl DaemonConfig {
    pub fn new(bind_address: SocketAddr, auth_key: impl Into<String>) -> Result<Self, ConfigError> {
        let auth_key = validate_auth_key(auth_key.into())?;
        let workspace_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Ok(Self {
            bind_address,
            public_url: None,
            data_dir: PathBuf::from(DEFAULT_DATA_DIR),
            auth_key,
            tls: None,
            qr_enabled: true,
            skills_enabled: true,
            subagents_enabled: true,
            session_title_profile_id: None,
            workspace_dir,
            global_skill_dirs: default_global_skill_dirs(),
            workspace_skill_dirs: DEFAULT_WORKSPACE_SKILLS_DIRS
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            outbound_proxy: OutboundProxyConfig::default(),
        })
    }

    pub fn with_data_dir(mut self, data_dir: impl Into<PathBuf>) -> Self {
        self.data_dir = data_dir.into();
        self
    }

    pub fn with_bind_address(mut self, bind_address: SocketAddr) -> Self {
        self.bind_address = bind_address;
        self
    }

    pub fn with_public_url(mut self, public_url: impl Into<String>) -> Result<Self, ConfigError> {
        self.public_url = Some(validate_public_url(public_url.into())?);
        Ok(self)
    }

    pub fn with_tls(
        mut self,
        certificate_file: impl Into<PathBuf>,
        private_key_file: impl Into<PathBuf>,
    ) -> Result<Self, ConfigError> {
        self.tls = Some(TlsConfig::new(certificate_file, private_key_file)?);
        Ok(self)
    }

    pub fn with_skills_enabled(mut self, enabled: bool) -> Self {
        self.skills_enabled = enabled;
        self
    }

    pub fn with_qr_enabled(mut self, enabled: bool) -> Self {
        self.qr_enabled = enabled;
        self
    }

    pub fn with_subagents_enabled(mut self, enabled: bool) -> Self {
        self.subagents_enabled = enabled;
        self
    }

    pub fn with_session_title_profile_id(
        mut self,
        profile_id: impl Into<String>,
    ) -> Result<Self, ConfigError> {
        self.session_title_profile_id = Some(normalize_profile_id(
            SESSION_TITLE_PROFILE_ID_ENV,
            profile_id.into(),
        )?);
        Ok(self)
    }

    pub fn with_workspace_dir(mut self, workspace_dir: impl Into<PathBuf>) -> Self {
        self.workspace_dir = normalize_workspace_dir(workspace_dir.into());
        self
    }

    pub fn with_global_skill_dirs(
        mut self,
        directories: impl IntoIterator<Item = PathBuf>,
    ) -> Self {
        self.global_skill_dirs = directories.into_iter().collect();
        self
    }

    pub fn with_workspace_skill_dirs(
        mut self,
        directories: impl IntoIterator<Item = PathBuf>,
    ) -> Self {
        self.workspace_skill_dirs = directories.into_iter().collect();
        self
    }

    pub fn from_env() -> Result<Self, ConfigError> {
        let value = environment_or_default(BIND_ADDRESS_ENV, DEFAULT_BIND_ADDRESS)?;
        let bind_address = value
            .parse()
            .map_err(|source| ConfigError::InvalidBindAddress { value, source })?;
        let data_dir = environment_or_default(DATA_DIR_ENV, DEFAULT_DATA_DIR)?;
        if data_dir.trim().is_empty() {
            return Err(ConfigError::InvalidDataDirectory { value: data_dir });
        }
        let outbound_proxy = OutboundProxyConfig::from_env()?;
        let home = home_directory();
        let auth_key = load_auth_key(optional_environment(AUTH_KEY_FILE_ENV)?, home.as_deref())?;
        let mut config = Self::new(bind_address, auth_key)?.with_data_dir(data_dir);
        if let Some(value) = optional_environment(PUBLIC_URL_ENV)? {
            config.public_url = Some(validate_public_url(value)?);
        }
        config.outbound_proxy = outbound_proxy;
        config.tls = tls_config_from_values(
            optional_environment(TLS_CERT_FILE_ENV)?,
            optional_environment(TLS_KEY_FILE_ENV)?,
        )?;
        if let Some(value) = optional_environment(SKILLS_ENABLED_ENV)? {
            config.skills_enabled = parse_boolean(SKILLS_ENABLED_ENV, &value)?;
        }
        if let Some(value) = optional_environment(SUBAGENTS_ENABLED_ENV)? {
            config.subagents_enabled = parse_boolean(SUBAGENTS_ENABLED_ENV, &value)?;
        }
        if let Some(value) = optional_environment(SESSION_TITLE_PROFILE_ID_ENV)? {
            config.session_title_profile_id =
                Some(normalize_profile_id(SESSION_TITLE_PROFILE_ID_ENV, value)?);
        }
        if let Some(value) = optional_environment(WORKSPACE_DIR_ENV)? {
            if value.trim().is_empty() {
                return Err(ConfigError::InvalidDirectory {
                    name: WORKSPACE_DIR_ENV,
                    value,
                });
            }
            config.workspace_dir = normalize_workspace_dir(PathBuf::from(value));
        }
        if let Some(value) = optional_environment_os(GLOBAL_SKILLS_DIRS_ENV) {
            config.global_skill_dirs = parse_path_list(value);
        }
        if let Some(value) = optional_environment_os(WORKSPACE_SKILLS_DIRS_ENV) {
            config.workspace_skill_dirs = parse_path_list(value);
        }
        Ok(config)
    }

    pub fn bind_address(&self) -> SocketAddr {
        self.bind_address
    }

    pub fn public_url(&self) -> Option<&str> {
        self.public_url.as_deref()
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn tls_config(&self) -> Option<&TlsConfig> {
        self.tls.as_ref()
    }

    pub fn skills_enabled(&self) -> bool {
        self.skills_enabled
    }

    pub fn qr_enabled(&self) -> bool {
        self.qr_enabled
    }

    pub fn subagents_enabled(&self) -> bool {
        self.subagents_enabled
    }

    pub fn session_title_profile_id(&self) -> Option<&str> {
        self.session_title_profile_id.as_deref()
    }

    pub fn workspace_dir(&self) -> &Path {
        &self.workspace_dir
    }

    pub fn global_skill_dirs(&self) -> &[PathBuf] {
        &self.global_skill_dirs
    }

    pub fn workspace_skill_dirs(&self) -> &[PathBuf] {
        &self.workspace_skill_dirs
    }

    pub fn skills_config(&self) -> SkillsConfig {
        self.skills_config_template()
            .resolve_against(&phi::Workspace::new(self.workspace_dir.clone()))
    }

    pub(crate) fn skills_config_template(&self) -> SkillsConfig {
        let mut config = SkillsConfig::new()
            .enabled(self.skills_enabled)
            .duplicate_policy(DuplicateSkillPolicy::LastWins);
        for path in &self.global_skill_dirs {
            config = config.skill_directory(SkillDirectory::new(path.clone()).source("global"));
        }
        for path in &self.workspace_skill_dirs {
            config = config.skill_directory(SkillDirectory::new(path.clone()).source("workspace"));
        }
        config
    }

    pub(crate) fn auth_key(&self) -> &str {
        &self.auth_key
    }

    pub(crate) fn provider_http_client(&self) -> Result<reqwest::Client, ConfigError> {
        self.outbound_proxy.http_client()
    }
}

impl fmt::Debug for DaemonConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DaemonConfig")
            .field("bind_address", &self.bind_address)
            .field("public_url", &self.public_url)
            .field("data_dir", &self.data_dir)
            .field("tls", &self.tls)
            .field("qr_enabled", &self.qr_enabled)
            .field("skills_enabled", &self.skills_enabled)
            .field("subagents_enabled", &self.subagents_enabled)
            .field("session_title_profile_id", &self.session_title_profile_id)
            .field("workspace_dir", &self.workspace_dir)
            .field("global_skill_dirs", &self.global_skill_dirs)
            .field("workspace_skill_dirs", &self.workspace_skill_dirs)
            .field("outbound_proxy", &self.outbound_proxy)
            .field("auth_key", &"[REDACTED]")
            .finish()
    }
}

fn environment_or_default(name: &'static str, default: &str) -> Result<String, ConfigError> {
    match env::var(name) {
        Ok(value) => Ok(value),
        Err(env::VarError::NotPresent) => Ok(default.to_owned()),
        Err(source) => Err(ConfigError::Environment { name, source }),
    }
}

fn optional_environment(name: &'static str) -> Result<Option<String>, ConfigError> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(source) => Err(ConfigError::Environment { name, source }),
    }
}

fn optional_environment_with_alias(
    name: &'static str,
    alias: &'static str,
) -> Result<Option<String>, ConfigError> {
    match optional_environment(name)? {
        Some(value) => Ok(Some(value)),
        None => optional_environment(alias),
    }
}

fn normalize_proxy_url(
    name: &'static str,
    value: Option<String>,
) -> Result<Option<String>, ConfigError> {
    let value = normalize_optional_environment(value);
    if let Some(value) = &value {
        reqwest::Proxy::all(value).map_err(|_| ConfigError::InvalidProxyUrl { name })?;
    }
    Ok(value)
}

fn normalize_optional_environment(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn validate_public_url(value: String) -> Result<String, ConfigError> {
    let value = value.trim();
    let url = reqwest::Url::parse(value).map_err(|_| ConfigError::InvalidPublicUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.has_host()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ConfigError::InvalidPublicUrl);
    }

    Ok(url.as_str().trim_end_matches('/').to_owned())
}

fn optional_environment_os(name: &'static str) -> Option<std::ffi::OsString> {
    env::var_os(name)
}

fn parse_path_list(value: std::ffi::OsString) -> Vec<PathBuf> {
    env::split_paths(&value)
        .filter(|path| !path.as_os_str().is_empty())
        .collect()
}

fn parse_boolean(name: &'static str, value: &str) -> Result<bool, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::InvalidBoolean {
            name,
            value: value.to_owned(),
        }),
    }
}

fn tls_config_from_values(
    certificate_file: Option<String>,
    private_key_file: Option<String>,
) -> Result<Option<TlsConfig>, ConfigError> {
    match (certificate_file, private_key_file) {
        (None, None) => Ok(None),
        (Some(certificate_file), Some(private_key_file)) => {
            TlsConfig::new(certificate_file, private_key_file).map(Some)
        }
        (None, Some(_)) => Err(ConfigError::IncompleteTlsConfiguration {
            missing: TLS_CERT_FILE_ENV,
        }),
        (Some(_), None) => Err(ConfigError::IncompleteTlsConfiguration {
            missing: TLS_KEY_FILE_ENV,
        }),
    }
}

fn validate_tls_file_path(name: &'static str, path: PathBuf) -> Result<PathBuf, ConfigError> {
    let is_blank =
        path.as_os_str().is_empty() || path.to_str().is_some_and(|value| value.trim().is_empty());
    if is_blank {
        return Err(ConfigError::InvalidTlsFilePath { name });
    }
    Ok(path)
}

fn normalize_profile_id(name: &'static str, value: String) -> Result<String, ConfigError> {
    let profile_id = value.trim();
    if profile_id.is_empty() || profile_id.len() > 128 || profile_id.chars().any(char::is_control) {
        return Err(ConfigError::InvalidProfileId { name, value });
    }
    Ok(profile_id.to_owned())
}

fn default_global_skill_dirs() -> Vec<PathBuf> {
    home_directory()
        .map(|home| home.join(DEFAULT_GLOBAL_SKILLS_DIR))
        .into_iter()
        .collect()
}

fn normalize_workspace_dir(path: PathBuf) -> PathBuf {
    phi::Workspace::new(path).root().to_owned()
}

fn home_directory() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("USERPROFILE")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
}

fn load_auth_key(
    configured_path: Option<String>,
    home_directory: Option<&Path>,
) -> Result<String, ConfigError> {
    if let Some(path) = configured_path {
        if path.trim().is_empty() {
            return Err(ConfigError::InvalidAuthKeyFilePath);
        }
        return read_auth_key(Path::new(&path));
    }

    let home_directory = home_directory.ok_or(ConfigError::HomeDirectoryUnavailable)?;
    load_or_create_default_auth_key(&home_directory.join(DEFAULT_AUTH_KEY_FILE))
}

fn load_or_create_default_auth_key(path: &Path) -> Result<String, ConfigError> {
    match read_auth_key(path) {
        Ok(key) => Ok(key),
        Err(ConfigError::AuthKeyFile { source, .. })
            if source.kind() == io::ErrorKind::NotFound =>
        {
            create_default_auth_key(path)
        }
        Err(error) => Err(error),
    }
}

fn create_default_auth_key(path: &Path) -> Result<String, ConfigError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder
            .create(parent)
            .map_err(|source| ConfigError::AuthKeyDirectory {
                path: parent.to_owned(),
                source,
            })?;
    }

    let key = generate_auth_key()?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);

    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            return read_auth_key(path);
        }
        Err(source) => {
            return Err(ConfigError::AuthKeyFileInitialization {
                path: path.to_owned(),
                source,
            });
        }
    };
    let result = file
        .write_all(key.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all());
    if let Err(source) = result {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(ConfigError::AuthKeyFileInitialization {
            path: path.to_owned(),
            source,
        });
    }
    Ok(key)
}

fn generate_auth_key() -> Result<String, ConfigError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut random = [0_u8; GENERATED_AUTH_KEY_BYTES];
    getrandom::fill(&mut random).map_err(|_| ConfigError::AuthKeyGeneration)?;
    let mut key = String::with_capacity(GENERATED_AUTH_KEY_BYTES * 2);
    for byte in random {
        key.push(char::from(HEX[usize::from(byte >> 4)]));
        key.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(key)
}

fn read_auth_key(path: &Path) -> Result<String, ConfigError> {
    let contents = fs::read_to_string(path).map_err(|source| ConfigError::AuthKeyFile {
        path: path.to_owned(),
        source,
    })?;
    let key = contents.trim_end_matches(['\r', '\n']).to_owned();
    validate_auth_key(key)
}

fn validate_auth_key(key: String) -> Result<String, ConfigError> {
    if key.len() < MIN_AUTH_KEY_BYTES
        || key.len() > MAX_AUTH_KEY_BYTES
        || !key.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(ConfigError::InvalidAuthKey);
    }
    Ok(key)
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not read environment variable {name}: {source}")]
    Environment {
        name: &'static str,
        #[source]
        source: env::VarError,
    },

    #[error("invalid daemon bind address {value:?}: {source}")]
    InvalidBindAddress {
        value: String,
        #[source]
        source: std::net::AddrParseError,
    },

    #[error("daemon data directory must not be empty (got {value:?})")]
    InvalidDataDirectory { value: String },

    #[error(
        "PHI_DAEMON_PUBLIC_URL must be an absolute HTTP(S) URL without credentials, query, or fragment"
    )]
    InvalidPublicUrl,

    #[error("daemon directory environment variable {name} must not be empty (got {value:?})")]
    InvalidDirectory { name: &'static str, value: String },

    #[error("daemon boolean environment variable {name} has invalid value {value:?}")]
    InvalidBoolean { name: &'static str, value: String },

    #[error("daemon proxy environment variable {name} is not a valid proxy URL")]
    InvalidProxyUrl { name: &'static str },

    #[error("could not initialize daemon Provider HTTP client")]
    ProviderHttpClientInitialization,

    #[error("daemon Provider profile environment variable {name} has invalid value {value:?}")]
    InvalidProfileId { name: &'static str, value: String },

    #[error("daemon auth key file path must not be empty")]
    InvalidAuthKeyFilePath,

    #[error(
        "could not determine the home directory for the default daemon auth key; set PHI_DAEMON_AUTH_KEY_FILE"
    )]
    HomeDirectoryUnavailable,

    #[error("could not create daemon auth key directory {path}: {source}")]
    AuthKeyDirectory {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("secure randomness is unavailable for daemon auth key generation")]
    AuthKeyGeneration,

    #[error("could not initialize daemon auth key file {path}: {source}")]
    AuthKeyFileInitialization {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error(
        "PHI_DAEMON_TLS_CERT_FILE and PHI_DAEMON_TLS_KEY_FILE must be configured together (missing {missing})"
    )]
    IncompleteTlsConfiguration { missing: &'static str },

    #[error("daemon TLS file path {name} must not be empty")]
    InvalidTlsFilePath { name: &'static str },

    #[error("could not read daemon auth key file {path}: {source}")]
    AuthKeyFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("daemon auth key must contain 32 to 4096 printable non-whitespace ASCII bytes")]
    InvalidAuthKey,
}

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::*;

    async fn capture_proxy_request(listener: TcpListener, response: &'static [u8]) -> String {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        stream.write_all(response).await.unwrap();
        String::from_utf8(request).unwrap()
    }

    #[test]
    fn builder_defaults_to_loopback_data_directory() {
        let config = DaemonConfig::new(
            DEFAULT_BIND_ADDRESS.parse().unwrap(),
            "a-secure-test-key-with-at-least-32-bytes",
        )
        .unwrap();
        assert_eq!(config.bind_address().to_string(), DEFAULT_BIND_ADDRESS);
        assert!(config.bind_address().ip().is_loopback());
        assert_eq!(config.public_url(), None);
        assert_eq!(config.data_dir(), Path::new(DEFAULT_DATA_DIR));
        assert_eq!(config.tls_config(), None);
        assert!(config.qr_enabled());
        assert!(config.skills_enabled());
        assert!(config.subagents_enabled());
        assert_eq!(config.session_title_profile_id(), None);
        assert!(
            config
                .global_skill_dirs()
                .iter()
                .all(|path| path.ends_with(DEFAULT_GLOBAL_SKILLS_DIR))
        );
        assert_eq!(
            config.workspace_skill_dirs(),
            &DEFAULT_WORKSPACE_SKILLS_DIRS.map(PathBuf::from)
        );
        assert_eq!(config.outbound_proxy, OutboundProxyConfig::default());
    }

    #[test]
    fn proxy_configuration_validates_urls_and_redacts_credentials() {
        let http_secret = "http-proxy-secret";
        let https_secret = "https-proxy-secret";
        let outbound_proxy = OutboundProxyConfig::from_values(
            Some(format!(
                "http://proxy-user:{http_secret}@http-proxy.example:8080"
            )),
            Some(format!(
                "http://proxy-user:{https_secret}@https-proxy.example:8080"
            )),
            None,
            Some("localhost,127.0.0.1".to_owned()),
        )
        .unwrap();
        let mut config = DaemonConfig::new(
            DEFAULT_BIND_ADDRESS.parse().unwrap(),
            "a-secure-test-key-with-at-least-32-bytes",
        )
        .unwrap();
        config.outbound_proxy = outbound_proxy;

        config.provider_http_client().unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains(http_secret));
        assert!(!debug.contains(https_secret));
        assert!(debug.contains("http_proxy_configured: true"));
        assert!(debug.contains("https_proxy_configured: true"));
        assert!(debug.contains("no_proxy_configured: true"));

        let invalid_secret = "invalid-proxy-secret";
        let error = OutboundProxyConfig::from_values(
            Some(format!("http://proxy-user:{invalid_secret}@[")),
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ConfigError::InvalidProxyUrl {
                name: HTTP_PROXY_ENV
            }
        ));
        assert!(!error.to_string().contains(invalid_secret));

        assert_eq!(
            OutboundProxyConfig::from_values(
                Some("  ".to_owned()),
                Some(String::new()),
                None,
                None,
            )
            .unwrap(),
            OutboundProxyConfig::default()
        );
    }

    #[tokio::test]
    async fn protocol_specific_proxy_settings_route_http_and_https_requests() {
        let http_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_proxy_url = format!("http://{}", http_listener.local_addr().unwrap());
        let http_capture = tokio::spawn(capture_proxy_request(
            http_listener,
            b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        ));
        let http_client = OutboundProxyConfig::from_values(Some(http_proxy_url), None, None, None)
            .unwrap()
            .http_client()
            .unwrap();

        let response = http_client
            .get("http://provider.example.invalid/v1/models")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let request = http_capture.await.unwrap();
        assert!(request.starts_with("GET http://provider.example.invalid/v1/models HTTP/1.1\r\n"));

        let https_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let https_proxy_url = format!("http://{}", https_listener.local_addr().unwrap());
        let https_capture = tokio::spawn(capture_proxy_request(
            https_listener,
            b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        ));
        let https_client =
            OutboundProxyConfig::from_values(None, Some(https_proxy_url), None, None)
                .unwrap()
                .http_client()
                .unwrap();

        assert!(
            https_client
                .get("https://provider.example.invalid/v1/models")
                .send()
                .await
                .is_err()
        );
        let request = https_capture.await.unwrap();
        assert!(request.starts_with("CONNECT provider.example.invalid:443 HTTP/1.1\r\n"));
    }

    #[test]
    fn builder_overrides_data_directory() {
        let config = DaemonConfig::new(
            DEFAULT_BIND_ADDRESS.parse().unwrap(),
            "a-secure-test-key-with-at-least-32-bytes",
        )
        .unwrap()
        .with_data_dir("state/phi");
        assert_eq!(config.data_dir(), Path::new("state/phi"));
    }

    #[test]
    fn builder_overrides_the_bind_address() {
        let config = DaemonConfig::new(
            DEFAULT_BIND_ADDRESS.parse().unwrap(),
            "a-secure-test-key-with-at-least-32-bytes",
        )
        .unwrap()
        .with_bind_address("0.0.0.0:9000".parse().unwrap());

        assert_eq!(config.bind_address(), "0.0.0.0:9000".parse().unwrap());
    }

    #[test]
    fn builder_validates_and_normalizes_the_public_url() {
        let config = DaemonConfig::new(
            DEFAULT_BIND_ADDRESS.parse().unwrap(),
            "a-secure-test-key-with-at-least-32-bytes",
        )
        .unwrap()
        .with_public_url("  HTTPS://PHI.EXAMPLE.COM/daemon/  ")
        .unwrap();

        assert_eq!(config.public_url(), Some("https://phi.example.com/daemon"));

        for invalid in [
            "",
            "phi.example.com",
            "ftp://phi.example.com",
            "https://user:secret@phi.example.com",
            "https://phi.example.com?token=secret",
            "https://phi.example.com#fragment",
        ] {
            let error = DaemonConfig::new(
                DEFAULT_BIND_ADDRESS.parse().unwrap(),
                "a-secure-test-key-with-at-least-32-bytes",
            )
            .unwrap()
            .with_public_url(invalid)
            .unwrap_err();
            assert!(matches!(error, ConfigError::InvalidPublicUrl));
            assert!(!error.to_string().contains("secret"));
        }
    }

    #[test]
    fn builder_can_disable_the_connection_qr_code() {
        let config = DaemonConfig::new(
            DEFAULT_BIND_ADDRESS.parse().unwrap(),
            "a-secure-test-key-with-at-least-32-bytes",
        )
        .unwrap()
        .with_qr_enabled(false);

        assert!(!config.qr_enabled());
    }

    #[test]
    fn builder_configures_tls_certificate_and_private_key_files() {
        let config = DaemonConfig::new(
            DEFAULT_BIND_ADDRESS.parse().unwrap(),
            "a-secure-test-key-with-at-least-32-bytes",
        )
        .unwrap()
        .with_tls("localhost.crt", "localhost.key")
        .unwrap();

        let tls = config.tls_config().unwrap();
        assert_eq!(tls.certificate_file(), Path::new("localhost.crt"));
        assert_eq!(tls.private_key_file(), Path::new("localhost.key"));
    }

    #[test]
    fn tls_certificate_and_private_key_must_be_configured_together() {
        assert!(matches!(
            tls_config_from_values(Some("localhost.crt".to_owned()), None),
            Err(ConfigError::IncompleteTlsConfiguration {
                missing: TLS_KEY_FILE_ENV
            })
        ));
        assert!(matches!(
            tls_config_from_values(None, Some("localhost.key".to_owned())),
            Err(ConfigError::IncompleteTlsConfiguration {
                missing: TLS_CERT_FILE_ENV
            })
        ));
        assert!(matches!(
            tls_config_from_values(Some("  ".to_owned()), Some("localhost.key".to_owned())),
            Err(ConfigError::InvalidTlsFilePath {
                name: TLS_CERT_FILE_ENV
            })
        ));
    }

    #[test]
    fn builder_configures_a_dedicated_session_title_profile() {
        let config = DaemonConfig::new(
            DEFAULT_BIND_ADDRESS.parse().unwrap(),
            "a-secure-test-key-with-at-least-32-bytes",
        )
        .unwrap()
        .with_session_title_profile_id(" titles ")
        .unwrap();

        assert_eq!(config.session_title_profile_id(), Some("titles"));
        assert!(matches!(
            DaemonConfig::new(
                DEFAULT_BIND_ADDRESS.parse().unwrap(),
                "a-secure-test-key-with-at-least-32-bytes",
            )
            .unwrap()
            .with_session_title_profile_id(" \n "),
            Err(ConfigError::InvalidProfileId {
                name: SESSION_TITLE_PROFILE_ID_ENV,
                ..
            })
        ));
    }

    #[test]
    fn debug_redacts_auth_key() {
        let secret = "canary-auth-key-that-must-never-appear";
        let private_key_path = "canary-private-key-path";
        let config = DaemonConfig::new(DEFAULT_BIND_ADDRESS.parse().unwrap(), secret)
            .unwrap()
            .with_tls("localhost.crt", private_key_path)
            .unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains(secret));
        assert!(!debug.contains(private_key_path));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn skills_config_resolves_multiple_directories_against_workspace() {
        let config = DaemonConfig::new(
            DEFAULT_BIND_ADDRESS.parse().unwrap(),
            "a-secure-test-key-with-at-least-32-bytes",
        )
        .unwrap()
        .with_workspace_dir("/workspace")
        .with_global_skill_dirs([PathBuf::from("/global/a"), PathBuf::from("global-b")])
        .with_workspace_skill_dirs([PathBuf::from(".phy/skills"), PathBuf::from("extra")]);

        let skills = config.skills_config();
        assert_eq!(skills.directories.len(), 4);
        assert_eq!(skills.directories[0].path, Path::new("/global/a"));
        assert_eq!(skills.directories[1].path, Path::new("/workspace/global-b"));
        assert_eq!(
            skills.directories[2].path,
            Path::new("/workspace/.phy/skills")
        );
        assert_eq!(skills.directories[3].path, Path::new("/workspace/extra"));
        assert_eq!(skills.directories[0].source.as_deref(), Some("global"));
        assert_eq!(skills.directories[3].source.as_deref(), Some("workspace"));
    }

    #[test]
    fn path_lists_support_multiple_platform_paths_and_empty_disables_them() {
        let joined = env::join_paths(["first", "second"]).unwrap();
        assert_eq!(
            parse_path_list(joined),
            vec![PathBuf::from("first"), PathBuf::from("second")]
        );
        assert!(parse_path_list(std::ffi::OsString::new()).is_empty());
        assert!(!parse_boolean(SKILLS_ENABLED_ENV, "off").unwrap());
        assert!(parse_boolean(SKILLS_ENABLED_ENV, "YES").unwrap());
        assert!(!parse_boolean(SUBAGENTS_ENABLED_ENV, "false").unwrap());
    }

    #[test]
    fn rejects_short_or_whitespace_wrapped_auth_keys() {
        assert!(matches!(
            DaemonConfig::new(DEFAULT_BIND_ADDRESS.parse().unwrap(), "too-short"),
            Err(ConfigError::InvalidAuthKey)
        ));
        assert!(matches!(
            DaemonConfig::new(
                DEFAULT_BIND_ADDRESS.parse().unwrap(),
                " surrounding-whitespace-is-not-accepted "
            ),
            Err(ConfigError::InvalidAuthKey)
        ));
    }

    #[test]
    fn auth_key_file_accepts_one_trailing_line_ending() {
        let secret = "auth-key-loaded-from-a-protected-file";
        let path = env::temp_dir().join(format!("phi-auth-{}.key", uuid::Uuid::now_v7()));
        fs::write(&path, format!("{secret}\n")).unwrap();

        let loaded = read_auth_key(&path).unwrap();
        let _ = fs::remove_file(path);
        assert_eq!(loaded, secret);
    }

    #[test]
    fn missing_auth_key_configuration_creates_a_protected_default_key() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(DEFAULT_AUTH_KEY_FILE);

        let generated = load_auth_key(None, Some(home.path())).unwrap();

        assert_eq!(generated.len(), GENERATED_AUTH_KEY_BYTES * 2);
        assert!(generated.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(read_auth_key(&path).unwrap(), generated);
        assert_eq!(load_auth_key(None, Some(home.path())).unwrap(), generated);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let file_mode = fs::metadata(&path).unwrap().permissions().mode();
            let directory_mode = fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(file_mode & 0o077, 0);
            assert_eq!(directory_mode & 0o077, 0);
        }
    }

    #[test]
    fn default_auth_key_initialization_never_overwrites_an_existing_key() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(DEFAULT_AUTH_KEY_FILE);
        let secret = "existing-auth-key-that-must-be-preserved";
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, format!("{secret}\n")).unwrap();

        assert_eq!(load_auth_key(None, Some(home.path())).unwrap(), secret);
        assert_eq!(fs::read_to_string(path).unwrap(), format!("{secret}\n"));
    }

    #[test]
    fn invalid_default_auth_key_is_reported_without_replacement() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(DEFAULT_AUTH_KEY_FILE);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "too-short\n").unwrap();

        assert!(matches!(
            load_auth_key(None, Some(home.path())),
            Err(ConfigError::InvalidAuthKey)
        ));
        assert_eq!(fs::read_to_string(path).unwrap(), "too-short\n");
    }

    #[test]
    fn explicit_auth_key_path_still_requires_an_existing_valid_file() {
        let home = tempfile::tempdir().unwrap();
        let missing = home.path().join("missing.key");

        assert!(matches!(
            load_auth_key(Some("  ".to_owned()), None),
            Err(ConfigError::InvalidAuthKeyFilePath)
        ));
        assert!(matches!(
            load_auth_key(Some(missing.to_string_lossy().into_owned()), None),
            Err(ConfigError::AuthKeyFile { source, .. })
                if source.kind() == io::ErrorKind::NotFound
        ));
        assert!(matches!(
            load_auth_key(None, None),
            Err(ConfigError::HomeDirectoryUnavailable)
        ));
    }
}
