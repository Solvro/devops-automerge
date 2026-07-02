use cloneable_errors::{ErrorContext, bail};
use octocrab::models::webhook_events::{
    WebhookEvent,
    payload::{PullRequestWebhookEventAction, PullRequestWebhookEventPayload},
};

use crate::{
    config::AppConfig,
    utils::{
        automerge::debounced_update_automerge, pull_request::PullRequestIdentifier,
        rules::classify_user,
    },
};

pub(super) async fn process_pr_event(
    config: AppConfig,
    event: &WebhookEvent,
    payload: &PullRequestWebhookEventPayload,
) -> Result<(), ErrorContext> {
    // update the pr num -> id cache
    if let Some(ref repo) = event.repository
        && let Some(ref repo_id) = repo.node_id
    {
        config.pull_request_id_cache.set(
            PullRequestIdentifier {
                repository_id: repo_id.clone(),
                pull_request_number: payload.pull_request.number,
            },
            &payload.pull_request.node_id,
        );
    }

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

    // debounced upate
    let Some(ref installation) = event.installation else {
        bail!("No installation data present on webhook payload");
    };
    debounced_update_automerge(&config, installation.id(), &payload.pull_request.node_id).await;

    Ok(())
}
