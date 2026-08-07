use fns_protocol::WorkspacePath;
use globset::{GlobBuilder, GlobMatcher};

use crate::FsError;

pub const DEFAULT_INCLUDES: &[&str] = &["**"];
pub const DEFAULT_EXCLUDES: &[&str] = &[
    ".git/**",
    ".hg/**",
    ".svn/**",
    "node_modules/**",
    ".venv/**",
    "venv/**",
    "target/**",
    "build/**",
    "dist/**",
    ".next/**",
    ".cache/**",
    "**/__pycache__/**",
    "**/.pytest_cache/**",
    "**/.mypy_cache/**",
    "**/.ruff_cache/**",
    "**/*.log",
    "**/*.sock",
    "**/*.pid",
    "**/.DS_Store",
    "**/.fns_state.json",
];
pub const HARD_INTERNAL_EXCLUDES: &[&str] = &[
    ".fns_state.json",
    "**/.fns_state.json",
    ".fns-tmp-*",
    "**/.fns-tmp-*",
    ".fns-delete-*",
    "**/.fns-delete-*",
    ".fns-internal/**",
    "**/.fns-internal/**",
];
pub const HARD_SECRET_EXCLUDES: &[&str] = &[
    ".env",
    "**/.env",
    ".env.*",
    "**/.env.*",
    "*.pem",
    "**/*.pem",
    "*.key",
    "**/*.key",
    ".ssh/**",
    "**/.ssh/**",
    "id_rsa",
    "**/id_rsa",
    "id_ed25519",
    "**/id_ed25519",
    ".aws/credentials",
    "**/.aws/credentials",
    ".config/gcloud/application_default_credentials.json",
    "**/.config/gcloud/application_default_credentials.json",
    ".azure/accessTokens.json",
    "**/.azure/accessTokens.json",
    ".azure/msal_token_cache.json",
    "**/.azure/msal_token_cache.json",
    ".kube/config",
    "**/.kube/config",
    ".docker/config.json",
    "**/.docker/config.json",
];
pub const SAFE_ENV_BASENAMES: &[&str] = &[".env.example", ".env.sample", ".env.template"];

const ENV_SECRET_PATTERN_COUNT: usize = 4;
const INVALID_RULE_REASON: &str = "invalid glob pattern";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncRuleConfig {
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
    pub protect_secrets: bool,
}

impl Default for SyncRuleConfig {
    fn default() -> Self {
        Self {
            includes: Vec::new(),
            excludes: Vec::new(),
            protect_secrets: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleSource {
    HardInternal,
    HardSecurity,
    UserExclude,
    UserInclude,
    DefaultExclude,
    DefaultInclude,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuleDecision {
    pub included: bool,
    pub source: RuleSource,
}

pub struct SyncRules {
    hard_internal: Vec<GlobMatcher>,
    hard_env_secrets: Vec<GlobMatcher>,
    hard_other_secrets: Vec<GlobMatcher>,
    user_excludes: Vec<GlobMatcher>,
    user_includes: Vec<GlobMatcher>,
    default_excludes: Vec<GlobMatcher>,
    default_includes: Vec<GlobMatcher>,
    user_include_prefixes: Vec<String>,
    protect_secrets: bool,
}

impl SyncRules {
    pub fn compile(config: SyncRuleConfig) -> Result<Self, FsError> {
        let user_include_prefixes = config
            .includes
            .iter()
            .map(|pattern| literal_prefix(pattern))
            .collect();

        Ok(Self {
            hard_internal: compile_globs(HARD_INTERNAL_EXCLUDES.iter().copied())?,
            hard_env_secrets: compile_globs(
                HARD_SECRET_EXCLUDES[..ENV_SECRET_PATTERN_COUNT]
                    .iter()
                    .copied(),
            )?,
            hard_other_secrets: compile_globs(
                HARD_SECRET_EXCLUDES[ENV_SECRET_PATTERN_COUNT..]
                    .iter()
                    .copied(),
            )?,
            user_excludes: compile_globs(config.excludes.iter().map(String::as_str))?,
            user_includes: compile_globs(config.includes.iter().map(String::as_str))?,
            default_excludes: compile_globs(DEFAULT_EXCLUDES.iter().copied())?,
            default_includes: compile_globs(DEFAULT_INCLUDES.iter().copied())?,
            user_include_prefixes,
            protect_secrets: config.protect_secrets,
        })
    }

    pub fn decide(&self, path: &WorkspacePath, is_dir: bool) -> RuleDecision {
        let value = path.as_str();

        if matches_any(&self.hard_internal, value, is_dir) {
            return excluded(RuleSource::HardInternal);
        }
        if self.protect_secrets
            && (matches_any(&self.hard_other_secrets, value, is_dir)
                || (!has_safe_env_basename(value)
                    && matches_any(&self.hard_env_secrets, value, is_dir)))
        {
            return excluded(RuleSource::HardSecurity);
        }
        if matches_user_rule(&self.user_excludes, value, is_dir) {
            return excluded(RuleSource::UserExclude);
        }
        if matches_user_rule(&self.user_includes, value, is_dir) {
            return included(RuleSource::UserInclude);
        }
        if matches_any(&self.default_excludes, value, is_dir) {
            return excluded(RuleSource::DefaultExclude);
        }

        RuleDecision {
            included: matches_any(&self.default_includes, value, is_dir),
            source: RuleSource::DefaultInclude,
        }
    }

    pub fn should_descend(&self, path: &WorkspacePath) -> bool {
        match self.decide(path, true) {
            RuleDecision {
                source:
                    RuleSource::HardInternal | RuleSource::HardSecurity | RuleSource::UserExclude,
                ..
            } => false,
            RuleDecision { included: true, .. } => true,
            RuleDecision {
                source: RuleSource::DefaultExclude,
                ..
            } => self
                .user_include_prefixes
                .iter()
                .any(|prefix| is_ancestor(path.as_str(), prefix)),
            _ => false,
        }
    }
}

fn compile_globs<'a>(
    patterns: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<GlobMatcher>, FsError> {
    patterns
        .into_iter()
        .enumerate()
        .map(|(index, pattern)| {
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .backslash_escape(false)
                .build()
                .map(|glob| glob.compile_matcher())
                .map_err(|_| FsError::InvalidRule {
                    index,
                    reason: INVALID_RULE_REASON.to_owned(),
                })
        })
        .collect()
}

fn matches_any(globs: &[GlobMatcher], path: &str, is_dir: bool) -> bool {
    globs.iter().any(|glob| glob.is_match(path))
        || (is_dir && {
            let directory_path = format!("{path}/");
            globs
                .iter()
                .any(|glob| glob.is_match(directory_path.as_str()))
        })
}

fn matches_user_rule(globs: &[GlobMatcher], path: &str, is_dir: bool) -> bool {
    matches_any(globs, path, is_dir)
        || safe_env_alias(path).is_some_and(|alias| matches_any(globs, &alias, is_dir))
}

fn has_safe_env_basename(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|basename| SAFE_ENV_BASENAMES.contains(&basename))
}

fn safe_env_alias(path: &str) -> Option<String> {
    has_safe_env_basename(path).then(|| {
        let basename = path
            .rsplit('/')
            .next()
            .expect("workspace paths are non-empty");
        format!("{}.env", &path[..path.len() - basename.len()])
    })
}

fn literal_prefix(pattern: &str) -> String {
    let end = pattern
        .char_indices()
        .find_map(|(index, character)| matches!(character, '*' | '?' | '[' | '{').then_some(index))
        .unwrap_or(pattern.len());
    pattern[..end].to_owned()
}

fn is_ancestor(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    !prefix.is_empty()
        && (path == prefix
            || prefix
                .strip_prefix(path)
                .is_some_and(|suffix| suffix.starts_with('/')))
}

const fn included(source: RuleSource) -> RuleDecision {
    RuleDecision {
        included: true,
        source,
    }
}

const fn excluded(source: RuleSource) -> RuleDecision {
    RuleDecision {
        included: false,
        source,
    }
}
