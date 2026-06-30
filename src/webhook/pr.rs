use cloneable_errors::{ErrorContext, ResContext, bail};
use graphql_client::GraphQLQuery;
use octocrab::models::webhook_events::{
    WebhookEvent,
    payload::{PullRequestWebhookEventAction, PullRequestWebhookEventPayload},
};

use crate::{
    automerge::update_automerge,
    config::AppConfig,
    graphql::{PullRequestQuery, pull_request_query},
    rules::{check_automerge_eligibility, classify_user},
};

pub(super) async fn process_pr_event(
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
