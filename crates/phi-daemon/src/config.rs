use std::{
    env, fmt, fs, io,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use thiserror::Error;

use phi::{DuplicateSkillPolicy, SkillDirectory, SkillsConfig};

pub const BIND_ADDRESS_ENV: &str = "PHI_DAEMON_BIND";
pub const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:8787";
pub const DATA_DIR_ENV: &str = "PHI_DAEMON_DATA_DIR";
pub const DEFAULT_DATA_DIR: &str = ".phi/daemon";
pub const AUTH_KEY_FILE_ENV: &str = "PHI_DAEMON_AUTH_KEY_FILE";
pub const SKILLS_ENABLED_ENV: &str = "PHI_DAEMON_SKILLS_ENABLED";
pub const SUBAGENTS_ENABLED_ENV: &str = "PHI_DAEMON_SUBAGENTS_ENABLED";
pub const WORKSPACE_DIR_ENV: &str = "PHI_DAEMON_WORKSPACE_DIR";
pub const GLOBAL_SKILLS_DIRS_ENV: &str = "PHI_DAEMON_GLOBAL_SKILLS_DIRS";
pub const WORKSPACE_SKILLS_DIRS_ENV: &str = "PHI_DAEMON_WORKSPACE_SKILLS_DIRS";
pub const DEFAULT_GLOBAL_SKILLS_DIR: &str = ".phy/skills";
pub const DEFAULT_WORKSPACE_SKILLS_DIRS: [&str; 2] = [".phy/skills", ".claude/skills"];

const MIN_AUTH_KEY_BYTES: usize = 32;
const MAX_AUTH_KEY_BYTES: usize = 4096;

#[derive(Clone, PartialEq, Eq)]
pub struct DaemonConfig {
    bind_address: SocketAddr,
    data_dir: PathBuf,
    auth_key: String,
    skills_enabled: bool,
    subagents_enabled: bool,
    workspace_dir: PathBuf,
    global_skill_dirs: Vec<PathBuf>,
    workspace_skill_dirs: Vec<PathBuf>,
}

impl DaemonConfig {
    pub fn new(bind_address: SocketAddr, auth_key: impl Into<String>) -> Result<Self, ConfigError> {
        let auth_key = validate_auth_key(auth_key.into())?;
        let workspace_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Ok(Self {
            bind_address,
            data_dir: PathBuf::from(DEFAULT_DATA_DIR),
            auth_key,
            skills_enabled: true,
            subagents_enabled: true,
            workspace_dir,
            global_skill_dirs: default_global_skill_dirs(),
            workspace_skill_dirs: DEFAULT_WORKSPACE_SKILLS_DIRS
                .into_iter()
                .map(PathBuf::from)
                .collect(),
        })
    }

    pub fn with_data_dir(mut self, data_dir: impl Into<PathBuf>) -> Self {
        self.data_dir = data_dir.into();
        self
    }

    pub fn with_skills_enabled(mut self, enabled: bool) -> Self {
        self.skills_enabled = enabled;
        self
    }

    pub fn with_subagents_enabled(mut self, enabled: bool) -> Self {
        self.subagents_enabled = enabled;
        self
    }

    pub fn with_workspace_dir(mut self, workspace_dir: impl Into<PathBuf>) -> Self {
        self.workspace_dir = workspace_dir.into();
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
        let auth_key_file = required_environment(AUTH_KEY_FILE_ENV)?;
        if auth_key_file.trim().is_empty() {
            return Err(ConfigError::InvalidAuthKeyFilePath);
        }
        let auth_key = read_auth_key(Path::new(&auth_key_file))?;
        let mut config = Self::new(bind_address, auth_key)?.with_data_dir(data_dir);
        if let Some(value) = optional_environment(SKILLS_ENABLED_ENV)? {
            config.skills_enabled = parse_boolean(SKILLS_ENABLED_ENV, &value)?;
        }
        if let Some(value) = optional_environment(SUBAGENTS_ENABLED_ENV)? {
            config.subagents_enabled = parse_boolean(SUBAGENTS_ENABLED_ENV, &value)?;
        }
        if let Some(value) = optional_environment(WORKSPACE_DIR_ENV)? {
            if value.trim().is_empty() {
                return Err(ConfigError::InvalidDirectory {
                    name: WORKSPACE_DIR_ENV,
                    value,
                });
            }
            config.workspace_dir = PathBuf::from(value);
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

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn skills_enabled(&self) -> bool {
        self.skills_enabled
    }

    pub fn subagents_enabled(&self) -> bool {
        self.subagents_enabled
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
        let mut config = SkillsConfig::new()
            .enabled(self.skills_enabled)
            .duplicate_policy(DuplicateSkillPolicy::LastWins);
        for path in &self.global_skill_dirs {
            config = config.skill_directory(
                SkillDirectory::new(self.resolve_from_workspace(path)).source("global"),
            );
        }
        for path in &self.workspace_skill_dirs {
            config = config.skill_directory(
                SkillDirectory::new(self.resolve_from_workspace(path)).source("workspace"),
            );
        }
        config
    }

    fn resolve_from_workspace(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_owned()
        } else {
            self.workspace_dir.join(path)
        }
    }

    pub(crate) fn auth_key(&self) -> &str {
        &self.auth_key
    }
}

impl fmt::Debug for DaemonConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DaemonConfig")
            .field("bind_address", &self.bind_address)
            .field("data_dir", &self.data_dir)
            .field("skills_enabled", &self.skills_enabled)
            .field("subagents_enabled", &self.subagents_enabled)
            .field("workspace_dir", &self.workspace_dir)
            .field("global_skill_dirs", &self.global_skill_dirs)
            .field("workspace_skill_dirs", &self.workspace_skill_dirs)
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

fn default_global_skill_dirs() -> Vec<PathBuf> {
    home_directory()
        .map(|home| home.join(DEFAULT_GLOBAL_SKILLS_DIR))
        .into_iter()
        .collect()
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

fn required_environment(name: &'static str) -> Result<String, ConfigError> {
    env::var(name).map_err(|source| ConfigError::Environment { name, source })
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

    #[error("daemon directory environment variable {name} must not be empty (got {value:?})")]
    InvalidDirectory { name: &'static str, value: String },

    #[error("daemon boolean environment variable {name} has invalid value {value:?}")]
    InvalidBoolean { name: &'static str, value: String },

    #[error("daemon auth key file path must not be empty")]
    InvalidAuthKeyFilePath,

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
    use super::*;

    #[test]
    fn builder_defaults_to_loopback_data_directory() {
        let config = DaemonConfig::new(
            DEFAULT_BIND_ADDRESS.parse().unwrap(),
            "a-secure-test-key-with-at-least-32-bytes",
        )
        .unwrap();
        assert_eq!(config.bind_address().to_string(), DEFAULT_BIND_ADDRESS);
        assert!(config.bind_address().ip().is_loopback());
        assert_eq!(config.data_dir(), Path::new(DEFAULT_DATA_DIR));
        assert!(config.skills_enabled());
        assert!(config.subagents_enabled());
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
    fn debug_redacts_auth_key() {
        let secret = "canary-auth-key-that-must-never-appear";
        let config = DaemonConfig::new(DEFAULT_BIND_ADDRESS.parse().unwrap(), secret).unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains(secret));
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
}
