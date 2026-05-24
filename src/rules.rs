use std::sync::Arc;

use octocrab::models::{Author, UserId};

use crate::{
    config::{AppConfig, AutomergeRule, DependabotRule},
    dependabot::parse_dependabot_commit,
    graphql::{actor_id, pull_request_query},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UserType {
    Dependabot,
    Unknown,
}

pub fn classify_user(user: &Author) -> UserType {
    #[allow(clippy::match_same_arms)]
    match (
        user.login.as_str(),
        user.id,
        user.node_id.as_str(),
        user.r#type.as_str(),
    ) {
        ("dependabot[bot]", UserId(49_699_333), "MDM6Qm90NDk2OTkzMzM=", "Bot") => {
            UserType::Dependabot
        }
        ("dependabot[bot]", ..) => UserType::Unknown,
        _ => UserType::Unknown,
    }
}

#[derive(Clone)]
pub enum EligibleResult<'a> {
    /// failed a hardcoded check - fail reason
    FailedHardChecks(&'static str),
    /// successfully matched a rule - rule name
    FoundRule(&'a AutomergeRule),
    /// no rules matched - list of rule - fail reason, must match repository name to appear
    NoRuleFound(Vec<(Option<Arc<str>>, &'static str)>),
}

impl<'a> EligibleResult<'a> {
    pub fn ok(&self) -> Option<&'a AutomergeRule> {
        match self {
            Self::FoundRule(x) => Some(x),
            _ => None,
        }
    }
}

#[allow(clippy::too_many_lines)]
pub fn check_automerge_eligibility<'a>(
    config: &'a AppConfig,
    response: &pull_request_query::ResponseData,
) -> EligibleResult<'a> {
    // hard-coded restrictions
    let Some(pull_request_query::PullRequestQueryNode::PullRequest(ref pull_request)) =
        response.node
    else {
        // pr not found lol
        return EligibleResult::FailedHardChecks("PR not found???");
    };
    // not open
    if !matches!(
        pull_request.state,
        pull_request_query::PullRequestState::OPEN
    ) {
        return EligibleResult::FailedHardChecks("PR is not open");
    }
    // no author
    let Some(ref pr_author) = pull_request.author else {
        return EligibleResult::FailedHardChecks("PR has no author");
    };
    // no commits
    if pull_request.commits.total_count <= 0 {
        return EligibleResult::FailedHardChecks("PR has no commits");
    }
    // over 100 commits
    if pull_request.commits.total_count >= 100 {
        return EligibleResult::FailedHardChecks("PR more than 100 commits");
    }
    // some commits not present
    if pull_request
        .commits
        .nodes
        .as_ref()
        .is_none_or(|x| x.iter().any(Option::is_none))
    {
        return EligibleResult::FailedHardChecks("Some commits could not be fetched");
    }
    // could not diff
    let Some(ref files_changed) = pull_request.files else {
        return EligibleResult::FailedHardChecks("Could not diff PR");
    };
    // no files
    if files_changed.total_count <= 0 {
        return EligibleResult::FailedHardChecks("PR has no changes");
    }
    // over 100 files
    if files_changed.total_count >= 100 {
        return EligibleResult::FailedHardChecks("PR changes more than 100 files");
    }
    // some files not present
    if files_changed
        .nodes
        .as_ref()
        .is_none_or(|x| x.iter().any(Option::is_none))
    {
        return EligibleResult::FailedHardChecks("Some file diffs could not be fetched");
    }
    // some file changes not of type "modified"
    if files_changed
        .nodes
        .iter()
        .flatten()
        .flatten()
        .any(|x| !matches!(x.change_type, pull_request_query::PatchStatus::MODIFIED))
    {
        return EligibleResult::FailedHardChecks(
            "Some file changes are of a different type than MODIFIED",
        );
    }

    // try to match a rule
    let mut failures = Vec::<(Option<Arc<str>>, &'static str)>::new();

    for rule in &*config.rules {
        if !rule
            .repositories
            .iter()
            .any(|x| **x == *pull_request.repository.name_with_owner)
        {
            continue;
        }
        if let Some(ref branches) = rule.branches
            && !branches.iter().any(|x| **x == *pull_request.base_ref_name)
        {
            failures.push((rule.name.clone(), "Ineligible PR base branch"));
            continue;
        }
        if let Some(max_commits) = rule.max_commits
            && pull_request.commits.total_count > max_commits.into()
        {
            failures.push((rule.name.clone(), "Too many commits"));
            continue;
        }
        if let Some(max_files) = rule.max_files
            && files_changed.total_count > max_files.into()
        {
            failures.push((rule.name.clone(), "Too many changed files"));
            continue;
        }
        if let Some(max_lines) = rule.max_changed_lines
            && pull_request.additions.unsigned_abs() + pull_request.deletions.unsigned_abs()
                > max_lines
        {
            failures.push((rule.name.clone(), "Too many changes"));
            continue;
        }
        if let Some(ref allowed_paths) = rule.allowed_paths
            && !files_changed
                .nodes
                .iter()
                .flatten()
                .flatten()
                .all(|file| allowed_paths.iter().any(|x| x.matches(&file.path)))
        {
            failures.push((
                rule.name.clone(),
                "PR changes file outside of allowed paths",
            ));
            continue;
        }

        // dependabot
        if let Some(ref dependabot) = rule.dependabot
            && pr_author.login == "dependabot"
        {
            match match_dependabot_rule(dependabot, pull_request, pr_author) {
                Some(failure) => {
                    failures.push((rule.name.clone(), failure));
                    continue;
                }
                None => {
                    return EligibleResult::FoundRule(rule);
                }
            }
        }

        failures.push((
            rule.name.clone(),
            "Not authored by a bot configured for automerge",
        ));
    }

    EligibleResult::NoRuleFound(failures)
}

fn match_dependabot_rule(
    rule: &DependabotRule,
    pull_request: &pull_request_query::PullRequestQueryNodeOnPullRequest,
    pr_author: &pull_request_query::ActorProps,
) -> Option<&'static str> {
    // check id
    if actor_id(pr_author) != "MDM6Qm90NDk2OTkzMzM=" {
        return Some("Not a real dependabot PR");
    }
    // check commits
    for commit in pull_request
        .commits
        .nodes
        .iter()
        .flatten()
        .flatten()
        .map(|x| &x.commit)
    {
        if !(commit.author.as_ref().is_some_and(|author| {
            author.email.as_deref() == Some("49699333+dependabot[bot]@users.noreply.github.com")
                && author.name.as_deref() == Some("dependabot[bot]")
        }) && commit.committer.as_ref().is_some_and(|committer| {
            committer.email.as_deref() == Some("noreply@github.com")
                && committer.name.as_deref() == Some("GitHub")
        }) && commit.signature.as_ref().is_some_and(|signature| {
            signature.is_valid
                && matches!(
                    signature.state,
                    pull_request_query::GitSignatureState::VALID
                )
                && signature.was_signed_by_git_hub
                && signature.signer.as_ref().is_some_and(|signer| {
                    signer.login == "web-flow" && signer.id == "MDQ6VXNlcjE5ODY0NDQ3"
                })
        })) {
            return Some("One of the commits was not authored by dependabot");
        }

        let Some(metadata) = parse_dependabot_commit(&commit.message) else {
            return Some("Could not deserialize the metadata block of one of dependabot's commits");
        };

        for update in &metadata.updated_dependencies {
            if let Some(ref names) = rule.dependency_names
                && !names.iter().any(|x| **x == *update.dependency_name)
            {
                return Some(
                    "A dependency from outside of the allowed dependency names list was updated",
                );
            }
            if let Some(ref groups) = rule.dependency_groups
                && !groups
                    .iter()
                    .any(|x| x.as_deref() == update.dependency_group.as_deref())
            {
                return Some(
                    "A dependency from outside of the allowed dependency groups list was updated",
                );
            }
            if let Some(ref types) = rule.dependency_types
                && !types.iter().any(|x| **x == *update.dependency_type)
            {
                return Some(
                    "A dependency from outside of the allowed dependency types list was updated",
                );
            }
            if let Some(ref types) = rule.update_types
                && !types
                    .iter()
                    .any(|x| x.as_deref() == update.update_type.as_deref())
            {
                return Some("One of the update types was outside of the allowed list");
            }
        }
    }
    None
}
