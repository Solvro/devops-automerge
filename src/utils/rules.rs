use std::sync::Arc;

use octocrab::models::{Author, UserId};
use smallbitvec::SmallBitVec;
use tracing::debug;

use super::dependabot::parse_dependabot_commit;
use crate::{
    config::{AppConfig, AutomergeRule, DependabotRule},
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

type GQLCheckSuites<'a> = &'a [Option<
    pull_request_query::PullRequestQueryNodeOnPullRequestChecksNodesCommitCheckSuitesNodes,
>];

#[derive(Clone)]
struct CheckSummary {
    /// number of checks that passed
    passed: u8,
    /// number of checks that failed
    failed: u8,
    /// number of checks still pending
    pending: u8,
    /// a bitfield with a boolean value for each configured required check
    ///
    /// to verify whether a given required check passed, read from the same index as the check held
    /// in the `required_checks` array
    required: SmallBitVec,
}

fn summarize_checks(suites: GQLCheckSuites<'_>, required_checks: &[Box<str>]) -> CheckSummary {
    let mut summary = CheckSummary {
        passed: 0,
        failed: 0,
        pending: 0,
        required: SmallBitVec::from_elem(required_checks.len(), false),
    };

    suites
        .iter()
        .flatten()
        .filter_map(|x| x.check_runs.as_ref())
        .filter_map(|x| x.nodes.as_ref())
        .flatten()
        .flatten()
        .for_each(|run| {
            if matches!(run.status, pull_request_query::CheckStatusState::COMPLETED) {
                match run.conclusion {
                    None => {
                        summary.pending += 1;
                    }
                    Some(pull_request_query::CheckConclusionState::SUCCESS) => {
                        summary.passed += 1;
                        // also mark the check as passed in the required bitfield
                        if let Some(index) =
                            required_checks.iter().position(|name| **name == *run.name)
                        {
                            summary.required.set(index, true);
                        }
                    }
                    Some(
                        pull_request_query::CheckConclusionState::STALE
                        | pull_request_query::CheckConclusionState::SKIPPED
                        | pull_request_query::CheckConclusionState::NEUTRAL,
                    ) => {}
                    Some(_) => {
                        summary.failed += 1;
                    }
                }
            } else {
                summary.pending += 1;
            }
        });

    summary
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

    // get the last commit
    let Some(checks) = pull_request
        .checks
        .nodes
        .as_ref()
        .and_then(|n| n.first().and_then(Option::as_ref))
    else {
        return EligibleResult::FailedHardChecks("Failed to fetch last commit's checks");
    };

    let checks = &checks.commit.check_suites;

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

        // checks
        if let Some(ref check_rules) = rule.checks {
            let Some(checks) = checks.as_ref().and_then(|x| x.nodes.as_ref()) else {
                failures.push((rule.name.clone(), "Could not fetch check suite data"));
                continue;
            };

            let checks = summarize_checks(checks, &check_rules.required);

            if let Some(max) = check_rules.max_pending
                && checks.pending > max
            {
                failures.push((rule.name.clone(), "Too many pending checks"));
            }
            if let Some(max) = check_rules.max_failed
                && checks.failed > max
            {
                failures.push((rule.name.clone(), "Too many failed checks"));
            }
            if let Some(min) = check_rules.min_passed
                && checks.passed < min
            {
                failures.push((rule.name.clone(), "Not enough passed checks"));
            }
            if !checks.required.all_true() {
                failures.push((
                    rule.name.clone(),
                    "Required checks are still pending or have failed",
                ));
            }
        }

        // dependabot
        if let Some(ref dependabot) = rule.dependabot
            && pr_author.login == "dependabot"
        {
            if let Some(failure) = match_dependabot_rule(dependabot, pull_request, pr_author) {
                failures.push((rule.name.clone(), failure));
                continue;
            }

            debug!(
                "PR matched rule '{}', GQL data: {response:#?}",
                rule.name.as_deref().unwrap_or("unnamed rule")
            );
            return EligibleResult::FoundRule(rule);
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
                    .any(|x| **x == *update.dependency_group.as_deref().unwrap_or_default())
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
                    .any(|x| **x == *update.update_type.as_deref().unwrap_or_default())
            {
                return Some("One of the update types was outside of the allowed list");
            }
        }
    }
    None
}
