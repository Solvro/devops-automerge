use std::fmt::Write;

use cloneable_errors::{ErrorContext, ResContext, bail};
use graphql_client::GraphQLQuery;
use octocrab::{
    Octocrab,
    models::webhook_events::{
        WebhookEvent,
        WebhookEventPayload::{IssueComment, PullRequest},
        payload::{
            IssueCommentWebhookEventAction, IssueCommentWebhookEventPayload,
            PullRequestWebhookEventAction, PullRequestWebhookEventPayload,
        },
    },
};
use tracing::error;

use crate::{
    config::{AppConfig, AutomergeRule, SimpleMergeMethod, SplitMergeMethod},
    graphql::{
        AddComment, CheckPermission, DequeuePullRequest, DisableAutomerge, EnableAutomerge,
        EnqueuePullRequest, MergePullRequest, PullRequestQuery, actor_id, add_comment,
        check_permission, dequeue_pull_request, disable_automerge, enable_automerge,
        enqueue_pull_request, merge_pull_request, pull_request_query,
    },
    rules::{EligibleResult, check_automerge_eligibility, classify_user},
};

pub async fn process_webhook_event(
    config: AppConfig,
    event: Box<WebhookEvent>,
) -> Result<(), ErrorContext> {
    match event.specific {
        PullRequest(ref payload) => process_pr_event(config, &event, payload)
            .await
            .context("Error while processing PR event"),
        IssueComment(ref payload) => process_comment_event(config, &event, payload)
            .await
            .context("Error while processing comment event"),
        _ => Ok(()),
    }
}

async fn process_pr_event(
    config: AppConfig,
    event: &WebhookEvent,
    payload: &PullRequestWebhookEventPayload,
) -> Result<(), ErrorContext> {
    // action types we care about
    if !matches!(
        payload.action,
        PullRequestWebhookEventAction::ReadyForReview
            | PullRequestWebhookEventAction::Opened
            | PullRequestWebhookEventAction::Reopened
            | PullRequestWebhookEventAction::Synchronize
            | PullRequestWebhookEventAction::Edited
    ) {
        return Ok(());
    }
    // check if the repo is in our config - if not, quit immediately
    if event
        .repository
        .as_ref()
        .and_then(|repo| repo.full_name.as_ref())
        .is_none_or(|repo| {
            !config.has_possible_rule(repo, classify_user(&payload.pull_request.user))
        })
    {
        return Ok(());
    }

    // get a client for this installation
    let Some(ref installation) = event.installation else {
        bail!("No installation data present on webhook payload");
    };
    let client = config
        .get_installation_client(installation.id())
        .context("Failed to get a client for the installation")?;

    // fetch all required data for the PR
    let response: pull_request_query::ResponseData = client
        .graphql(&PullRequestQuery::build_query(
            pull_request_query::Variables {
                node_id: payload.pull_request.node_id.clone(),
            },
        ))
        .await
        .context("Failed to execute PullRequestQuery via GraphQL")?;

    // try to match a rule
    update_automerge(
        &client,
        &response,
        check_automerge_eligibility(&config, &response).ok(),
    )
    .await
    .context("Failed to enable/disable PR automerge")
}

async fn process_comment_event(
    config: AppConfig,
    event: &WebhookEvent,
    payload: &IssueCommentWebhookEventPayload,
) -> Result<(), ErrorContext> {
    // only care about new comments
    if payload.action != IssueCommentWebhookEventAction::Created {
        return Ok(());
    }

    // must be sent by user
    if event.sender.as_ref().is_none_or(|x| x.r#type != "User") {
        return Ok(());
    }

    // must contain body
    let Some(ref body) = payload.comment.body else {
        return Ok(());
    };

    match body.trim().to_lowercase().as_str() {
        "!automerge check" => process_check_command(config, event, payload)
            .await
            .context("Error while processing automerge check command"),
        _ => Ok(()),
    }
}

async fn process_check_command(
    config: AppConfig,
    event: &WebhookEvent,
    payload: &IssueCommentWebhookEventPayload,
) -> Result<(), ErrorContext> {
    // get a client for this installation
    let Some(ref installation) = event.installation else {
        bail!("No installation data present on webhook payload");
    };
    let client = config
        .get_installation_client(installation.id())
        .context("Failed to get a client for the installation")?;

    if !is_user_repo_admin(&client, event)
        .await
        .context("Failed to check comment sender's repository permissions")?
    {
        client
            .graphql::<add_comment::ResponseData>(&AddComment::build_query(
                add_comment::Variables {
                    id: payload.issue.node_id.clone(),
                    body: "You must be a repository admin to run commands!".into(),
                },
            ))
            .await
            .context("Failed to send permission denied comment")?;
        return Ok(());
    }

    // check if we're on a pr
    if payload.issue.pull_request.is_none() {
        client
            .graphql::<add_comment::ResponseData>(&AddComment::build_query(
                add_comment::Variables {
                    id: payload.issue.node_id.clone(),
                    body: "This command must be run on a pull request".into(),
                },
            ))
            .await
            .context("Failed to send error comment")?;
        return Ok(());
    }

    // fetch data
    let response: pull_request_query::ResponseData = client
        .graphql(&PullRequestQuery::build_query(
            pull_request_query::Variables {
                node_id: payload.issue.node_id.clone(),
            },
        ))
        .await
        .context("Failed to execute PullRequestQuery via GraphQL")?;

    // match rules
    let check_result = check_automerge_eligibility(&config, &response);

    // update PR
    let update_result = update_automerge(&client, &response, check_result.ok()).await;

    // post comment
    let mut comment = match check_result {
        EligibleResult::FoundRule(rule) => format!(
            "This PR is eligible for automerge based on {}.",
            if let Some(ref name) = rule.name {
                format!("rule `{name}`")
            } else {
                "an unnamed rule".into()
            }
        ),
        EligibleResult::FailedHardChecks(msg) => format!(
            "This PR is not eligible for automerge, because it failed a hardcoded check - {msg}."
        ),
        EligibleResult::NoRuleFound(failures) if failures.is_empty() => {
            "This PR is not eligible for automerge, because it could not match any rules.".into()
        }
        EligibleResult::NoRuleFound(failures) => {
            let mut res = "This PR is not eligible for automerge, because it could not match any rules.\nRule matches attempted:".to_string();
            for (name, reason) in failures {
                if let Some(name) = name {
                    write!(res, "\n- rule `{name}`: {reason}").unwrap();
                } else {
                    write!(res, "\n- an unnamed rule: {reason}").unwrap();
                }
            }
            res
        }
    };

    if let Err(e) = update_result {
        write!(
            comment,
            "\n\nEnabling automerge on the pull request failed:\n```\n{e:?}\n```"
        )
        .unwrap();
    }

    client
        .graphql::<add_comment::ResponseData>(&AddComment::build_query(add_comment::Variables {
            id: payload.issue.node_id.clone(),
            body: comment,
        }))
        .await
        .context("Failed to send result comment")?;
    Ok(())
}

async fn is_user_repo_admin(client: &Octocrab, event: &WebhookEvent) -> Result<bool, ErrorContext> {
    let response: check_permission::ResponseData = client
        .graphql(&CheckPermission::build_query(check_permission::Variables {
            repo_id: event
                .repository
                .as_ref()
                .context("No repository in event???")?
                .node_id
                .as_ref()
                .context("Repo has no node id???")?
                .clone(),
            login: event
                .sender
                .as_ref()
                .context("No sender in event???")?
                .login
                .clone(),
        }))
        .await
        .context("Failed to fetch sender repo permissions")?;

    let Some(check_permission::CheckPermissionNode::Repository(repo)) = response.node else {
        bail!("tried to fetch repo by node id, but didn't get it in response");
    };
    let Some(collaborators) = repo.collaborators else {
        bail!("got null collaborators");
    };
    let Some(edge) = collaborators.edges.iter().flatten().flatten().next() else {
        return Ok(false);
    };
    Ok(matches!(
        edge.permission,
        check_permission::RepositoryPermission::ADMIN,
    ))
}

#[allow(clippy::too_many_lines)]
async fn update_automerge(
    client: &Octocrab,
    response: &pull_request_query::ResponseData,
    rule: Option<&AutomergeRule>,
) -> Result<(), ErrorContext> {
    match rule {
        None => {
            // undo automerge/enqueue, if we triggered it
            if let Some(pull_request_query::PullRequestQueryNode::PullRequest(ref pull_request)) =
                response.node
            {
                if let Some(ref automerge) = pull_request.auto_merge_request
                && let Some(ref enabled_by) = automerge.enabled_by
                && actor_id(enabled_by) == response.viewer.id
                // disable automerge here
                && let Err(e) = client
                    .graphql::<disable_automerge::ResponseData>(
                        &DisableAutomerge::build_query(disable_automerge::Variables {
                            id: pull_request.id.clone(),
                        }),
                    )
                    .await
                {
                    error!(
                        "Failed to disable PR automerge after it lost automerge eligibility: {e:?}"
                    );
                }
                if let Some(ref merge_queue) = pull_request.merge_queue_entry
                && actor_id(&merge_queue.enqueuer) == response.viewer.id
                // dequeue here
                && let Err(e) = client
                    .graphql::<dequeue_pull_request::ResponseData>(
                        &DequeuePullRequest::build_query(dequeue_pull_request::Variables {
                            id: pull_request.id.clone(),
                        }),
                    )
                    .await
                {
                    error!("Failed to dequeue PR after it lost automerge eligibility: {e:?}");
                }
            }
        }
        Some(rule) => {
            let Some(pull_request_query::PullRequestQueryNode::PullRequest(ref pull_request)) =
                response.node
            else {
                return Ok(());
            };
            let head_id = pull_request.head_ref_oid.clone();
            // check if automerge/queue is already enabled
            if pull_request.auto_merge_request.is_some() || pull_request.merge_queue_entry.is_some()
            {
                return Ok(());
            }
            // merge
            match rule.merge_method.split() {
                SplitMergeMethod::Instant(method) => {
                    client
                        .graphql::<merge_pull_request::ResponseData>(
                            &MergePullRequest::build_query(merge_pull_request::Variables {
                                id: pull_request.id.clone(),
                                head_oid: head_id,
                                method: match method {
                                    SimpleMergeMethod::Merge => {
                                        merge_pull_request::PullRequestMergeMethod::MERGE
                                    }
                                    SimpleMergeMethod::Squash => {
                                        merge_pull_request::PullRequestMergeMethod::SQUASH
                                    }
                                    SimpleMergeMethod::Rebase => {
                                        merge_pull_request::PullRequestMergeMethod::REBASE
                                    }
                                },
                            }),
                        )
                        .await
                        .context("Failed to merge PR")?;
                }
                SplitMergeMethod::Auto(method) => {
                    client
                        .graphql::<enable_automerge::ResponseData>(&EnableAutomerge::build_query(
                            enable_automerge::Variables {
                                id: pull_request.id.clone(),
                                head_oid: head_id,
                                method: match method {
                                    SimpleMergeMethod::Merge => {
                                        enable_automerge::PullRequestMergeMethod::MERGE
                                    }
                                    SimpleMergeMethod::Squash => {
                                        enable_automerge::PullRequestMergeMethod::SQUASH
                                    }
                                    SimpleMergeMethod::Rebase => {
                                        enable_automerge::PullRequestMergeMethod::REBASE
                                    }
                                },
                            },
                        ))
                        .await
                        .context("Failed to enable automerge on PR")?;
                }
                SplitMergeMethod::Queue => {
                    client
                        .graphql::<enqueue_pull_request::ResponseData>(
                            &EnqueuePullRequest::build_query(enqueue_pull_request::Variables {
                                id: pull_request.id.clone(),
                                head_oid: head_id,
                            }),
                        )
                        .await
                        .context("Failed to add PR to merge queue")?;
                }
            }
        }
    }
    Ok(())
}
