use std::path::PathBuf;

pub const DEFAULT_MAX_SKILL_BYTES: usize = 1024 * 1024;
pub const DEFAULT_MAX_SKILLS: usize = 512;
pub const DEFAULT_SKILL_LISTING_BUDGET: usize = 8_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DuplicateSkillPolicy {
    Error,
    FirstWins,
    #[default]
    LastWins,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillDirectory {
    pub path: PathBuf,
    pub required: bool,
    pub source: Option<String>,
}

impl SkillDirectory {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            required: false,
            source: None,
        }
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillsConfig {
    pub enabled: bool,
    pub directories: Vec<SkillDirectory>,
    pub duplicate_policy: DuplicateSkillPolicy,
    pub strict: bool,
    pub follow_symlinks: bool,
    pub max_skill_bytes: usize,
    pub max_skills: usize,
    pub listing_char_budget: usize,
}

impl SkillsConfig {
    /// Creates an enabled skills configuration with no implicit directories.
    pub fn new() -> Self {
        Self {
            enabled: true,
            ..Self::default()
        }
    }

    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.directories.push(SkillDirectory::new(path));
        self
    }

    pub fn skill_directory(mut self, directory: SkillDirectory) -> Self {
        self.directories.push(directory);
        self
    }

    pub fn directories(mut self, directories: impl IntoIterator<Item = SkillDirectory>) -> Self {
        self.directories.extend(directories);
        self
    }

    pub fn duplicate_policy(mut self, policy: DuplicateSkillPolicy) -> Self {
        self.duplicate_policy = policy;
        self
    }

    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    pub fn follow_symlinks(mut self, follow: bool) -> Self {
        self.follow_symlinks = follow;
        self
    }

    pub fn max_skill_bytes(mut self, maximum: usize) -> Self {
        self.max_skill_bytes = maximum.max(1);
        self
    }

    pub fn max_skills(mut self, maximum: usize) -> Self {
        self.max_skills = maximum.max(1);
        self
    }

    pub fn listing_char_budget(mut self, budget: usize) -> Self {
        self.listing_char_budget = budget.max(1);
        self
    }
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            directories: Vec::new(),
            duplicate_policy: DuplicateSkillPolicy::LastWins,
            strict: false,
            follow_symlinks: false,
            max_skill_bytes: DEFAULT_MAX_SKILL_BYTES,
            max_skills: DEFAULT_MAX_SKILLS,
            listing_char_budget: DEFAULT_SKILL_LISTING_BUDGET,
        }
    }
}
