use std::{
    collections::{HashMap, hash_map::Entry},
    env, fs,
    sync::{Arc, Mutex},
};

use base64::Engine;
use cloneable_errors::{ErrorContext, ResContext, bail};
use jsonwebtoken::EncodingKey;
use octocrab::{
    Octocrab, OctocrabBuilder,
    models::{AppId, InstallationId},
};
use serde::{Deserialize, Deserializer, de::Visitor};

use crate::rules::UserType;

#[derive(Clone)]
pub struct AppConfig {
    /// HMAC key for validating webhook payloads
    pub webhook_secret: Arc<str>,
    /// Github client authenticated with app's credentials
    pub app_client: Arc<Octocrab>,
    /// Clients for each installation ID
    pub installation_clients: Arc<Mutex<HashMap<InstallationId, Arc<Octocrab>>>>,
    /// Automerge rules for deciding when to automatically merge a PR
    pub rules: Arc<[AutomergeRule]>,
}

impl From<&FileConfig> for AppConfig {
    fn from(value: &FileConfig) -> Self {
        Self {
            webhook_secret: value.app.webhook_secret.clone(),
            app_client: OctocrabBuilder::new()
                .app(AppId(value.app.app_id), value.app.private_key.clone())
                .build()
                .expect("Failed to build app client")
                .into(),
            installation_clients: Arc::default(),
            rules: value.rules.clone(),
        }
    }
}

impl AppConfig {
    /// get an octocrab client for a given installation ID
    pub fn get_installation_client(
        &self,
        installation_id: InstallationId,
    ) -> octocrab::Result<Arc<Octocrab>> {
        match self
            .installation_clients
            .lock()
            .expect("Installation clients map mutex is poisoned")
            .entry(installation_id)
        {
            Entry::Occupied(entry) => Ok(entry.get().clone()),
            Entry::Vacant(entry) => Ok(entry
                .insert(self.app_client.installation(installation_id)?.into())
                .clone()),
        }
    }

    /// check whether any automerge rule exists that could match for the given combination of
    /// repo + pr author
    ///
    /// intended as an early check for when we know the repo & pr author, but we don't have the
    /// commits or other PR metadata to fully verify,
    /// used as a check for whether it's worth fetching the full PR data
    pub fn has_possible_rule(&self, repo: &str, author_type: UserType) -> bool {
        if let UserType::Unknown = author_type {
            return false;
        }

        self.rules.iter().any(|rule| {
            rule.repositories.iter().any(|x| **x == *repo)
                && match author_type {
                    UserType::Unknown => false,
                    UserType::Dependabot => rule.dependabot.is_some(),
                }
        })
    }
}

#[derive(Deserialize)]
pub struct FileConfig {
    /// Service listening sockets config
    #[serde(default)]
    pub listen: ListenConfig,
    /// Github app credentials
    pub app: GithubAppConfig,
    /// Automerge rules for deciding when to automatically merge a PR
    #[serde(default)]
    pub rules: Arc<[AutomergeRule]>,
}

#[derive(Deserialize, Clone)]
pub struct GithubAppConfig {
    pub webhook_secret: Arc<str>,
    #[serde(deserialize_with = "deserialize_key")]
    pub private_key: EncodingKey,
    pub app_id: u64,
}

#[derive(Deserialize)]
pub struct AutomergeRule {
    /// optional name of the rule, used in debug command output
    #[serde(default)]
    pub name: Option<Arc<str>>,
    /// full repository names to apply this rule to
    pub repositories: Box<[Box<str>]>,
    /// merge method to use for PRs that match this rule
    pub merge_method: ConfigMergeMethod,
    /// base branch names eligible for automerge
    ///
    /// None = all branches are eligible
    /// empty array = no branches are eligible
    #[serde(default)]
    pub branches: Option<Box<[Box<str>]>>,
    /// path patterns that are allowed to be modified by PRs
    ///
    /// None = no restrictions,
    /// Some = only automerge PRs that only modify the specified paths
    pub allowed_paths: Option<Box<[Glob]>>,
    /// max amount of commits a PR can have to be eligible for automerge
    ///
    /// NOTE: All PRs must have between 1 and 100 (inclusive) commits to be eligible for automerge.
    ///       This option cannot override this requirement.
    #[serde(default)]
    pub max_commits: Option<u8>,
    /// max amount of files a PR can change to be eligible for automerge
    ///
    /// NOTE: All PRs must have between 1 and 100 (inclusive) files modified to be eligible for automerge.
    ///       This option cannot override this requirement.
    #[serde(default)]
    pub max_files: Option<u8>,
    /// max amount of changed lines (additions+deletions) a PR can have to be eligible for automerge
    ///
    /// None = no restriction
    #[serde(default)]
    pub max_changed_lines: Option<u64>,
    /// rules for auto-merging dependabot PRs
    ///
    /// None = never merge dependabot PRs
    ///
    /// NOTE: A PR must have been created by dependabot, and all commits must be authored by
    ///       dependabot and reported as validly signed by GitHub in order to match this rule.
    #[serde(default)]
    pub dependabot: Option<DependabotRule>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ConfigMergeMethod {
    /// Immediately merge the PR with a merge commit
    InstantMerge,
    /// Immediately merge the PR by rebasing onto the base branch
    InstantRebase,
    /// Immediately merge the PR by squashing all changes
    InstantSquash,
    /// Immediately add the PR to the merge queue
    AddToMergeQueue,
    /// Automatically merge the PR with a merge commit when branch protection rules allow it
    AutoMerge,
    /// Automatically merge the PR by rebasing when branch protection rules allow it
    AutoRebase,
    /// Automatically merge the PR by squashing when branch protection rules allow it
    AutoSquash,
}

impl ConfigMergeMethod {
    pub fn split(self) -> SplitMergeMethod {
        match self {
            Self::InstantMerge => SplitMergeMethod::Instant(SimpleMergeMethod::Merge),
            Self::InstantRebase => SplitMergeMethod::Instant(SimpleMergeMethod::Rebase),
            Self::InstantSquash => SplitMergeMethod::Instant(SimpleMergeMethod::Squash),
            Self::AutoMerge => SplitMergeMethod::Auto(SimpleMergeMethod::Merge),
            Self::AutoRebase => SplitMergeMethod::Auto(SimpleMergeMethod::Rebase),
            Self::AutoSquash => SplitMergeMethod::Auto(SimpleMergeMethod::Squash),
            Self::AddToMergeQueue => SplitMergeMethod::Queue,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitMergeMethod {
    Instant(SimpleMergeMethod),
    Auto(SimpleMergeMethod),
    Queue,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SimpleMergeMethod {
    Merge,
    Rebase,
    Squash,
}

#[derive(Deserialize)]
pub struct DependabotRule {
    /// dependencies that may be modified by dependabot PRs
    ///
    /// None = no restrictions
    /// Some = only automerge PRs that upgrade the specified dependency names
    ///
    /// WARNING: this check relies on data specified in dependabot's commit descriptions!
    ///          commit descriptions are only trusted if authored by dependabot and if github
    ///          vouches them as verified, but should github be compromised, this may merge
    ///          untrusted PRs!
    pub dependency_names: Option<Box<[Box<str>]>>,
    /// dependency groups that may be modified by dependabot PRs
    ///
    /// None = no restrictions
    /// Some = only automerge PRs that upgrade the specified dependency groups
    ///
    /// NOTE: to allow dependencies without a dependency group to be updated, specify null as
    /// one of the entries on this list
    /// WARNING: this check relies on data specified in dependabot's commit descriptions!
    ///          commit descriptions are only trusted if authored by dependabot and if github
    ///          vouches them as verified, but should github be compromised, this may merge
    ///          untrusted PRs!
    pub dependency_groups: Option<Box<[Option<Box<str>>]>>,
    /// "dependency-type"s that may be specified in dependabot PR commits
    ///
    /// None = no restrictions
    /// Some = only automerge PRs where all updates across all commits only specify the specified
    ///        "dependency-type"s
    ///
    /// WARNING: this check relies on data specified in dependabot's commit descriptions!
    ///          commit descriptions are only trusted if authored by dependabot and if github
    ///          vouches them as verified, but should github be compromised, this may merge
    ///          untrusted PRs!
    pub dependency_types: Option<Box<[Box<str>]>>,
    /// "update-type"s that may be specified in dependabot PR commits
    ///
    /// None = no restrictions
    /// Some = only automerge PRs where all updates across all commits only specify the specified
    ///        "update-type"s
    ///
    /// WARNING: this check relies on data specified in dependabot's commit descriptions!
    ///          commit descriptions are only trusted if authored by dependabot and if github
    ///          vouches them as verified, but should github be compromised, this may merge
    ///          untrusted PRs!
    pub update_types: Option<Box<[Box<str>]>>,
}

#[derive(Debug, Deserialize)]
pub struct ListenConfig {
    /// Listen on a TCP socket
    #[serde(default)]
    pub tcp: Option<Box<str>>,
    /// Listen on a unix socket
    #[serde(default)]
    pub unix: Option<Box<str>>,
    /// Mode for the unix socket
    #[serde(default = "default_unix_mode")]
    pub unix_mode: u32,
}

impl Default for ListenConfig {
    fn default() -> Self {
        Self {
            tcp: Some("[::]:8080".into()),
            unix: None,
            unix_mode: 0,
        }
    }
}

fn default_unix_mode() -> u32 {
    0o666
}

impl FileConfig {
    pub fn get() -> Result<Self, ErrorContext> {
        if let Some(config_text) = env::var_os("AUTOMERGE_CONFIG") {
            // config file in envvar
            toml::from_slice(config_text.as_encoded_bytes())
                .context("Failed to parse the contents of AUTOMERGE_CONFIG as FileConfig")
        } else if let Some(config_b64) = env::var_os("AUTOMERGE_CONFIG_B64") {
            // config file base64-encoded in envvar
            let decoded = base64::engine::general_purpose::URL_SAFE
                .decode(config_b64.as_encoded_bytes())
                .context("Failed to base64-decode the contents of AUTOMERGE_CONFIG_B64 envvar")?;

            toml::from_slice(&decoded).context("Failed to parse the base64-decoded contents of AUTOMERGE_CONFIG_B64 envvar as FileConfig")
        } else if let Some(config_path) = env::var_os("AUTOMERGE_CONFIG_FILE") {
            // path to config in envvar
            let file = fs::read(&config_path).with_context(|| {
                format!(
                    "Failed to read {} (AUTOMERGE_CONFIG_FILE)",
                    config_path.display()
                )
            })?;

            toml::from_slice(&file).with_context(|| {
                format!(
                    "Failed to parse the contents of {} (AUTOMERGE_CONFIG_FILE) as FileConfig",
                    config_path.display()
                )
            })
        } else {
            bail!(
                "None of the supported AUTOMERGE_CONFIG* envvars present - no app config available"
            );
        }
    }
}

fn deserialize_key<'de, D: Deserializer<'de>>(deserializer: D) -> Result<EncodingKey, D::Error> {
    struct KeyVisitor;
    impl Visitor<'_> for KeyVisitor {
        type Value = EncodingKey;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a PEM-encoded RSA private key")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            EncodingKey::from_rsa_pem(v.trim().as_bytes()).map_err(E::custom)
        }
    }

    deserializer.deserialize_str(KeyVisitor)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glob(glob::Pattern);

impl<'de> Deserialize<'de> for Glob {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct GlobVisitor;
        impl Visitor<'_> for GlobVisitor {
            type Value = Glob;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a valid unix-style path pattern (glob)")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                glob::Pattern::new(v).map_err(E::custom).map(Glob)
            }
        }

        deserializer.deserialize_str(GlobVisitor)
    }
}

impl Glob {
    pub fn matches(&self, other: &str) -> bool {
        if other.starts_with('/') {
            self.0.matches_with(
                other,
                glob::MatchOptions {
                    case_sensitive: true,
                    require_literal_separator: true,
                    require_literal_leading_dot: false,
                },
            )
        } else {
            self.0.matches_with(
                &format!("/{other}"),
                glob::MatchOptions {
                    case_sensitive: true,
                    require_literal_separator: true,
                    require_literal_leading_dot: false,
                },
            )
        }
    }
}
