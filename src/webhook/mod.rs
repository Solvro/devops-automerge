use std::mem::MaybeUninit;

use axum::http::HeaderMap;
use bytes::Bytes;
use cloneable_errors::{ErrorContext, ResContext};
use ctutils::CtEq;
use hmac::{Hmac, KeyInit, Mac};
use octocrab::models::webhook_events::{
    WebhookEvent,
    WebhookEventPayload::{CheckRun, IssueComment, PullRequest},
};
use sha2::Sha256;
use tracing::warn;

use crate::{
    config::AppConfig,
    webhook::{
        check_run::process_check_run_event, comment::process_comment_event, pr::process_pr_event,
    },
};

mod check_run;
mod comment;
mod pr;

type HmacSha256 = Hmac<Sha256>;

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
        CheckRun(ref payload) => process_check_run_event(config, &event, payload)
            .await
            .context("Error while processing check_run event"),
        _ => Ok(()),
    }
}

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
