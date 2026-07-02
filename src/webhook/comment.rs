use cloneable_errors::{ErrorContext, ResContext, bail};
use graphql_client::GraphQLQuery;
use octocrab::{
    Octocrab,
    models::webhook_events::{
        WebhookEvent,
        payload::{IssueCommentWebhookEventAction, IssueCommentWebhookEventPayload},
    },
};

use crate::{
    config::AppConfig,
    graphql::{
        AddComment, CheckPermission, PullRequestQuery, add_comment, check_permission,
        pull_request_query,
    },
    utils::{
        automerge::update_automerge,
        pull_request::PullRequestIdentifier,
        rules::{EligibleResult, check_automerge_eligibility},
    },
};

use std::fmt::Write;

pub(super) async fn process_comment_event(
    config: AppConfig,
    event: &WebhookEvent,
    payload: &IssueCommentWebhookEventPayload,
) -> Result<(), ErrorContext> {
    // update the pr num -> id cache
    if payload.issue.pull_request.is_some()
        && let Some(ref repo) = event.repository
        && let Some(ref repo_id) = repo.node_id
    {
        config.pull_request_id_cache.set(
            PullRequestIdentifier {
                repository_id: repo_id.clone(),
                pull_request_number: payload.issue.number,
            },
            &payload.issue.node_id,
        );
    }

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
            "This PR is not eligible for automerge because it failed a hardcoded check - {msg}."
        ),
        EligibleResult::NoRuleFound(failures) if failures.is_empty() => {
            "This PR is not eligible for automerge because it could not match any rules.".into()
        }
        EligibleResult::NoRuleFound(failures) => {
            let mut res = "This PR is not eligible for automerge because it could not match any rules.\nRule matches attempted:".to_string();
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
