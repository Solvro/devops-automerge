use std::mem::MaybeUninit;

use axum::http::HeaderMap;
use bytes::Bytes;
use cloneable_errors::{ErrorContext, ResContext};
use ctutils::CtEq;
use graphql_client::GraphQLQuery;
use hmac::{Hmac, KeyInit, Mac};
use octocrab::Octocrab;
use sha2::Sha256;
use tracing::warn;

use crate::{
    config::{ConfigMergeMethod, SimpleMergeMethod, SplitMergeMethod},
    graphql::{
        EnableAutomerge, EnqueuePullRequest, MergePullRequest, enable_automerge,
        enqueue_pull_request, merge_pull_request,
    },
};

type HmacSha256 = Hmac<Sha256>;

pub fn verify_webhook_payload(body: &Bytes, headers: &HeaderMap, webhook_secret: &str) -> bool {
    let Some(signature) = headers.get("X-Hub-Signature-256") else {
        warn!("Invalid POST /webhook: no X-Hub-Signature-256");
        return false;
    };
    let Ok(signature) = signature.to_str() else {
        warn!("Invalid POST /webhook: X-Hub-Signature-256 was not text");
        return false;
    };
    let Some(signature) = signature.strip_prefix("sha256=") else {
        warn!("Invalid POST /webhook: X-Hub-Signature-256 did not start with sha256");
        return false;
    };
    if !(signature.len() == 64
        && signature
            .chars()
            .all(|c| c.is_ascii_digit() || (c.is_ascii_lowercase() && c.is_ascii_hexdigit())))
    {
        warn!("Invalid POST /webhook: X-Hub-Signature-256 was not lowercase sha256");
        return false;
    }

    let signature = {
        let mut parsed: [MaybeUninit<u8>; 32] = [const { MaybeUninit::uninit() }; 32];
        for (i, el) in parsed.iter_mut().enumerate() {
            el.write(
                u8::from_str_radix(&signature[i * 2..=i * 2 + 1], 16)
                    .expect("signature was verified to be hex"),
            );
        }
        // SAFETY: we've just iterated over the entire array and initialized each element
        unsafe { MaybeUninit::<[u8; 32]>::from(parsed).assume_init() }
    };

    // calculate the hmac
    let mut mac = HmacSha256::new_from_slice(webhook_secret.as_bytes())
        .expect("Configured webhook secret is not a valid HMAC key???");
    mac.update(body);
    let mac = mac.finalize();

    // constant-time equality check
    let result: bool = mac.as_bytes().ct_eq(&signature).into();
    if !result {
        warn!("Invalid POST /webhook: signature mismatch");
    }
    result
}

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
