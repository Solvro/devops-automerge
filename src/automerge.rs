use cloneable_errors::ErrorContext;
use graphql_client::GraphQLQuery;
use octocrab::Octocrab;
use tracing::error;

use crate::{
    config::AutomergeRule,
    graphql::{
        DequeuePullRequest, DisableAutomerge, actor_id, dequeue_pull_request, disable_automerge,
        pull_request_query,
    },
    utils::merge_pull_request,
};

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
            merge_pull_request(client, rule.merge_method, pull_request.id.clone(), head_id).await?;
        }
    }
    Ok(())
}
