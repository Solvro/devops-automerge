use std::{
    collections::{HashMap, hash_map::Entry},
    sync::{Arc, Mutex},
};

use cloneable_errors::{ErrorContext, ResContext, bail};
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use graphql_client::GraphQLQuery;
use octocrab::Octocrab;

use crate::{
    config::{ConfigMergeMethod, SimpleMergeMethod, SplitMergeMethod},
    graphql::{
        EnableAutomerge, EnqueuePullRequest, MergePullRequest, PullRequestByNumber,
        enable_automerge, enqueue_pull_request, merge_pull_request,
        pull_request_by_number::{self, PullRequestByNumberNode},
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

#[derive(Default)]
pub struct PullRequestIdCache(Mutex<HashMap<PullRequestIdentifier, PullRequestIdCacheEntry>>);

#[derive(Clone)]
enum PullRequestIdCacheEntry {
    Ready(String),
    Error(ErrorContext),
    Pending(Shared<BoxFuture<'static, Result<String, ErrorContext>>>),
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PullRequestIdentifier {
    pub repository_id: String,
    pub pull_request_number: u64,
}

impl PullRequestIdCache {
    fn update(
        &self,
        identifier: PullRequestIdentifier,
        result: impl FnOnce() -> PullRequestIdCacheEntry,
    ) {
        match self
            .0
            .lock()
            .expect("PullRequestIdCache mutex is posioned")
            .entry(identifier)
        {
            Entry::Vacant(entry) => {
                entry.insert_entry(result());
            }
            Entry::Occupied(mut entry) => {
                if matches!(entry.get(), PullRequestIdCacheEntry::Ready(..)) {
                    // no need to overwrite
                    return;
                }
                entry.insert(result());
            }
        }
    }

    pub fn set(&self, identifier: PullRequestIdentifier, node_id: &str) {
        self.update(identifier, || {
            PullRequestIdCacheEntry::Ready(node_id.to_owned())
        });
    }

    pub async fn get(
        &self,
        client: &Arc<Octocrab>,
        identifier: PullRequestIdentifier,
    ) -> Result<String, ErrorContext> {
        let fut = match self
            .0
            .lock()
            .expect("PullRequestIdCache mutex is poisoned")
            .entry(identifier)
        {
            Entry::Occupied(entry) => match entry.get().clone() {
                PullRequestIdCacheEntry::Ready(res) => return Ok(res),
                PullRequestIdCacheEntry::Error(err) => return Err(err),
                PullRequestIdCacheEntry::Pending(fut) => fut.boxed(),
            },
            Entry::Vacant(entry) => {
                let identifier = entry.key().clone();
                let fut = Self::fetch(client.clone(), identifier.clone())
                    .boxed()
                    .shared();
                entry.insert(PullRequestIdCacheEntry::Pending(fut.clone()));

                async move {
                    let result = fut.await;
                    let new_entry = match result.clone() {
                        Ok(res) => PullRequestIdCacheEntry::Ready(res),
                        Err(err) => PullRequestIdCacheEntry::Error(err),
                    };
                    self.update(identifier, || new_entry);

                    result
                }
                .boxed()
            }
        };
        fut.await
    }

    async fn fetch(
        client: Arc<Octocrab>,
        identifier: PullRequestIdentifier,
    ) -> Result<String, ErrorContext> {
        let response: pull_request_by_number::ResponseData = client
            .graphql(&PullRequestByNumber::build_query(
                pull_request_by_number::Variables {
                    repository_id: identifier.repository_id,
                    pr_number: identifier.pull_request_number.cast_signed(),
                },
            ))
            .await
            .context("Failed to execute PullRequestByNumber via GraphQL")?;

        let PullRequestByNumberNode::Repository(repo) =
            response.node.context("Repository not found")?
        else {
            bail!("Provided repository node_id did not resolve to a repository object!");
        };

        Ok(repo.pull_request.context("Pull request not found")?.id)
    }
}
