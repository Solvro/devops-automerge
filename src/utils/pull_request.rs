use cloneable_errors::{ErrorContext, ResContext};
use graphql_client::GraphQLQuery;
use octocrab::Octocrab;

use crate::{
    config::{ConfigMergeMethod, SimpleMergeMethod, SplitMergeMethod},
    graphql::{
        EnableAutomerge, EnqueuePullRequest, MergePullRequest, enable_automerge,
        enqueue_pull_request, merge_pull_request,
    },
};

pub async fn merge_pull_request(
    client: &Octocrab,
    method: ConfigMergeMethod,
    pr_id: String,
    head_id: String,
) -> Result<(), ErrorContext> {
    match method.split() {
        SplitMergeMethod::Instant(method) => {
            client
                .graphql::<merge_pull_request::ResponseData>(&MergePullRequest::build_query(
                    merge_pull_request::Variables {
                        id: pr_id,
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
                    },
                ))
                .await
                .context("Failed to merge PR")?;
        }
        SplitMergeMethod::Auto(method) => {
            client
                .graphql::<enable_automerge::ResponseData>(&EnableAutomerge::build_query(
                    enable_automerge::Variables {
                        id: pr_id,
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
                .graphql::<enqueue_pull_request::ResponseData>(&EnqueuePullRequest::build_query(
                    enqueue_pull_request::Variables {
                        id: pr_id,
                        head_oid: head_id,
                    },
                ))
                .await
                .context("Failed to add PR to merge queue")?;
        }
    }
    Ok(())
}
