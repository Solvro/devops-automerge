use cloneable_errors::{ErrorContext, ResContext};
use octocrab::models::webhook_events::{WebhookEvent, payload::CheckRunWebhookEventPayload};
use serde::Deserialize;
use tokio::task::JoinSet;
use tracing::error;

use crate::{
    config::AppConfig,
    utils::{automerge::debounced_update_automerge, pull_request::PullRequestIdentifier},
};

#[derive(Deserialize)]
struct CheckRun {
    pull_requests: Vec<CheckRunPR>,
}

#[derive(Deserialize)]
struct CheckRunPR {
    number: u64,
}

pub(super) async fn process_check_run_event(
    config: AppConfig,
    event: &WebhookEvent,
    payload: &CheckRunWebhookEventPayload,
) -> Result<(), ErrorContext> {
    // parse payload
    let install = event
        .installation
        .as_ref()
        .context("Payload missing installation data")?;
    let repo_id = event
        .repository
        .as_ref()
        .context("Payload missing repo data")?
        .node_id
        .as_ref()
        .context("Repo data missing node id")?;
    let check_run = CheckRun::deserialize(&payload.check_run)
        .context("Failed to parse the check_run property of the webhook payload")?;

    // get installation client
    let installation_id = install.id();
    let client = config
        .get_installation_client(installation_id)
        .context("Failed to get installation client")?;

    // trigger updates for each pull request in the list of affected prs
    let mut join_set = JoinSet::new();

    for pr in check_run.pull_requests {
        let identifier = PullRequestIdentifier {
            pull_request_number: pr.number,
            repository_id: repo_id.clone(),
        };
        let config = config.clone();
        let client = client.clone();
        join_set.spawn(async move {
            if let Err::<(), ErrorContext>(e) = async {
                let pr_node_id = config
                    .pull_request_id_cache
                    .get(&client, identifier.clone())
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to fetch node id for pull request {} on repo id {}",
                            identifier.pull_request_number, identifier.repository_id
                        )
                    })?;

                debounced_update_automerge(&config, installation_id, &pr_node_id).await;
                Ok(())
            }
            .await
            {
                error!(
                    "Error while updating PR {} on repo id {} due to a check_run event: {e:?}",
                    identifier.pull_request_number, identifier.repository_id
                );
            }
        });
    }

    join_set.join_all().await;
    Ok(())
}
