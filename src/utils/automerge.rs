use std::{
    collections::{HashMap, hash_map::Entry},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use cloneable_errors::{ErrorContext, ResContext};
use graphql_client::GraphQLQuery;
use octocrab::{Octocrab, models::InstallationId};
use tokio::time::sleep;
use tracing::error;

use super::{pull_request::merge_pull_request, rules::check_automerge_eligibility};
use crate::{
    config::{AppConfig, AutomergeRule},
    graphql::{
        Approve, DequeuePullRequest, DisableAutomerge, DismissReview, PullRequestQuery, actor_id,
        approve, dequeue_pull_request, disable_automerge, dismiss_review, pull_request_query,
    },
};

/// a map of pr node id -> debounce flag, wrapped in a mutex
pub type AutomergeDebounceMap = Mutex<HashMap<String, Arc<AtomicBool>>>;

pub async fn debounced_update_automerge(
    config: &AppConfig,
    installation_id: InstallationId,
    node_id: &str,
) -> () {
    // try to register the PR in the debounce map
    let flag = match config
        .automerge_debounce_map
        .lock()
        .expect("debounce map lock poisoned")
        .entry(node_id.to_owned())
    {
        // already exists, set to true instead
        Entry::Occupied(entry) => {
            entry.get().store(true, Ordering::Relaxed);
            return;
        }
        // create and return
        Entry::Vacant(entry) => {
            let flag = Arc::<AtomicBool>::default();
            entry.insert_entry(flag.clone());
            flag
        }
    };

    // debounce: wait 5s, check flag, repeat until flag is false
    loop {
        sleep(Duration::from_secs(5)).await;
        if !flag.swap(false, Ordering::Relaxed) {
            break;
        }
    }

    // remove from map
    config
        .automerge_debounce_map
        .lock()
        .expect("debounce map lock poisoned")
        .remove(node_id);

    // fetch pr data and update automerge
    if let Err(e) = async {
        let client = config
            .get_installation_client(installation_id)
            .context("Failed to get a client for the installation")?;

        let response: pull_request_query::ResponseData = client
            .graphql(&PullRequestQuery::build_query(
                pull_request_query::Variables {
                    node_id: node_id.to_owned(),
                },
            ))
            .await
            .context("Failed to execute PullRequestQuery via GraphQL")?;

        update_automerge(
            &client,
            &response,
            check_automerge_eligibility(config, &response).ok(),
        )
        .await
        .context("Failed enable/disable automerge on PR")?;

        Ok::<(), ErrorContext>(())
    }
    .await
    {
        error!("Debounced automerge update failed: {e:?}");
    }
}

fn get_own_review<'a>(
    pull_request: &'a pull_request_query::PullRequestQueryNodeOnPullRequest,
    login: &'_ str,
) -> Option<&'a str> {
    pull_request
        .reviews
        .iter()
        .filter_map(|r| r.nodes.as_ref())
        .flatten()
        .flatten()
        .find(|r| r.author.as_ref().is_some_and(|a| a.login == login))
        .map(|r| r.id.as_str())
}

pub async fn update_automerge(
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
                if let Some(review_id) = get_own_review(pull_request, &response.viewer.login)
                    && let Err(e) = client
                        .graphql::<dismiss_review::ResponseData>(&DismissReview::build_query(
                            dismiss_review::Variables {
                                id: review_id.to_owned(),
                                message: "PR no longer matches any automerge rules".to_owned(),
                            },
                        ))
                        .await
                {
                    error!(
                        "Failed to dismiss own review after PR lost automerge eligibility: {e:?}"
                    );
                }
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
            // review if needed
            if rule.autoapprove
                && get_own_review(pull_request, &response.viewer.login).is_none()
                && let Err(e) = client
                    .graphql::<approve::ResponseData>(&Approve::build_query(approve::Variables {
                        id: pull_request.id.clone(),
                        head_id: head_id.clone(),
                        comment: format!(
                            "PR matches automerge rule '{}'",
                            rule.name.as_deref().unwrap_or("unnamed rule")
                        ),
                    }))
                    .await
            {
                error!("Failed to approve PR before automerging: {e:?}");
            }

            // check if automerge/queue is already enabled
            if pull_request.auto_merge_request.is_some() || pull_request.merge_queue_entry.is_some()
            {
                return Ok(());
            }
            // merge
            merge_pull_request(client, rule.merge_method, pull_request.id.clone(), head_id).await?;
        }
    }
    Ok(())
}
