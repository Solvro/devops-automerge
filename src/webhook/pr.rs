use cloneable_errors::{ErrorContext, bail};
use octocrab::models::webhook_events::{
    WebhookEvent,
    payload::{PullRequestWebhookEventAction, PullRequestWebhookEventPayload},
};

use crate::{automerge::debounced_update_automerge, config::AppConfig, rules::classify_user};

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

    // debounced upate
    let Some(ref installation) = event.installation else {
        bail!("No installation data present on webhook payload");
    };
    debounced_update_automerge(&config, installation.id(), &payload.pull_request.node_id).await;

    Ok(())
}
