use cloneable_errors::{ErrorContext, ResContext};
use octocrab::models::webhook_events::{
    WebhookEvent,
    WebhookEventPayload::{IssueComment, PullRequest},
};

use crate::{
    config::AppConfig,
    webhook::{comment::process_comment_event, pr::process_pr_event},
};

mod comment;
mod pr;

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
