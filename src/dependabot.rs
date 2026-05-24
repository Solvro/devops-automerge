use serde::Deserialize;
use tracing::error;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DependabotCommit {
    pub updated_dependencies: Vec<UpdatedDependency>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct UpdatedDependency {
    pub dependency_name: String,
    // pub dependency_version: String,
    pub dependency_type: String,
    pub update_type: Option<String>,
    pub dependency_group: Option<String>,
}

pub fn parse_dependabot_commit(commit: &str) -> Option<DependabotCommit> {
    let (_, metadata_block) = commit.split_once("---")?;
    let (metadata_block, _) = metadata_block.split_once("...")?;
    match yaml_serde::from_str(metadata_block) {
        Ok(x) => Some(x),
        Err(e) => {
            error!("Failed to deserialize dependabot's commit metadata block: {e:?}");
            None
        }
    }
}
