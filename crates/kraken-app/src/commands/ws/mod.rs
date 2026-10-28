//! The `kraken ws` command surface and dispatch.
//!
//! The surface mirrors the two families of the WS v2 protocol, as two
//! clap-flattened sub-enums: a `ChannelCommand` converts into the
//! `WsSubscription` it streams on the
//! shared [`StreamClient`](kraken_ws::StreamClient) ([`stream`]); a
//! `MethodCommand` converts into its typed params and runs as a one-shot
//! request over its own socket ([`request`]). [`execute`] routes between the
//! two; a newly added command must join a family — and thereby pick its driver
//! — to compile.
//!
//! Each variant carries a same-named args struct, and the CLI→wire boundary is
//! that struct's `From`/`TryFrom` impl: infallible for the channels (clap's
//! typed fields already pin the per-flag bounds), fallible for the methods that
//! carry cross-field rules clap cannot express (mutually-exclusive quantities,
//! batch-size and timeout bounds, required identifiers) — enforced before any
//! socket opens.
//!
//! Request ids: `--req-id` is one global flag on `kraken ws`, the client
//! request id Kraken echoes back so a caller can correlate acks and replies
//! against an id it chose. Zero is unrepresentable ([`NonZeroU64`]): an earlier
//! client design kept a shared monotonic id counter starting at 1 and reserved
//! 0 as its "no live id" sentinel — the type now carries that rule with no
//! runtime check. When omitted, a subscribe frame carries no `req_id` and a
//! one-shot method uses a fixed id of 1, safe because a `kraken ws`
//! process runs exactly one request on its own socket.

use crate::{
    cli::AppContext,
    errors::{ErrorCategory, KrakenError, Result},
    output::summary::Summarize,
    sink::{self, stdout::StdoutSink},
};
use itertools::Itertools;
use kraken_contract::ws::{MethodCommand, WsCommand};
use kraken_core::{
    AddOrderParams, AmendOrderParams, BatchAddParams, BatchCancelParams,
    CancelAllOrdersAfterParams, CancelAllParams, CancelOrderParams, PingParams, Request,
    WsSubscription,
};
use kraken_ws::RequestClient;
use std::num::NonZeroU64;

mod channels;

/// Entry point for `kraken ws`: route each family to its driver. `req_id` is
/// the global `--req-id` override.
pub(crate) async fn execute(
    cmd: WsCommand,
    req_id: Option<NonZeroU64>,
    ctx: &AppContext,
) -> Result<()> {
    match cmd {
        WsCommand::Channel(channel) => stream(channel.into(), req_id, ctx).await,
        WsCommand::Method(method) => request(method, req_id, ctx).await,
    }
}

// === Drivers and shared rules ===

/// Drive one subscription on the shared streaming client, rendering frames to
/// stdout.
async fn stream(sub: WsSubscription, req_id: Option<NonZeroU64>, ctx: &AppContext) -> Result<()> {
    crate::stream::run(
        vec![sub],
        req_id,
        None,
        StdoutSink::new(ctx.format),
        None,
        ctx,
    )
    .await
}

/// Run one method over the one-shot request engine — [`stream`]'s counterpart.
/// Each arm converts its args into the method's typed params and hands them to
/// [`run_method`].
///
/// One arm per method is intrinsic: each params type carries its own
/// `Response`, so the concrete type must be named here for [`run_method`] to
/// monomorphise — the [`Request`] trait is not dyn-compatible. This match is
/// the only place that dispatches on the method set; adding a method means
/// adding its arm.
async fn request(
    method: MethodCommand,
    req_id: Option<NonZeroU64>,
    ctx: &AppContext,
) -> Result<()> {
    match method {
        MethodCommand::AddOrder(args) => {
            run_method(AddOrderParams::try_from(args)?, req_id, ctx).await
        }
        MethodCommand::AmendOrder(args) => {
            run_method(AmendOrderParams::try_from(args)?, req_id, ctx).await
        }
        MethodCommand::CancelOrder(args) => {
            run_method(CancelOrderParams::try_from(args)?, req_id, ctx).await
        }
        MethodCommand::CancelAll(_) => run_method(CancelAllParams {}, req_id, ctx).await,
        MethodCommand::CancelAfter(args) => {
            run_method(CancelAllOrdersAfterParams::try_from(args)?, req_id, ctx).await
        }
        MethodCommand::BatchAdd(args) => {
            run_method(BatchAddParams::try_from(args)?, req_id, ctx).await
        }
        MethodCommand::BatchCancel(args) => {
            run_method(BatchCancelParams::try_from(args)?, req_id, ctx).await
        }
        MethodCommand::Ping(_) => run_method(PingParams {}, req_id, ctx).await,
    }
}

/// Submit one typed request over its own socket and render every correlated
/// reply.
async fn run_method<P: Request + Send + Sync + 'static>(
    params: P,
    req_id: Option<NonZeroU64>,
    ctx: &AppContext,
) -> Result<()>
where
    P::Response: Summarize,
{
    let cfg = ctx.ws_config()?;
    let fmt = ctx.format;
    let expected = params.expected_replies().get();
    // A `kraken ws <method>` process issues exactly one request on its own socket,
    // so a fixed default of 1 is a safe correlation id — no shared monotonic
    // counter needed.
    let batch = RequestClient::new(cfg)
        .send(params, req_id.unwrap_or(NonZeroU64::MIN))
        .await?;

    // One reply for most methods; a multi-id `cancel_order` streams one per id —
    // the request path gathers the whole batch up to the server's close and
    // types each outcome. Handle both uniformly — render every successful
    // reply, then report whatever failed or went missing: a truncated batch
    // arrives as `Ok` with its typed stop reason riding along, so an early
    // close or deadline surfaces here as a shortfall against `expected`, never
    // as silent success. A `success: false` reply arrives as a categorized server
    // error (auth, rate_limit, api, …) — the same classification the REST path
    // gives — and `batch_error` returns a lone failure verbatim, so a
    // single-reply rejection (including a whole-frame rejection, which arrives
    // as one id-less error reply) keeps its full, precise error rather than
    // being flattened into a batch summary.
    let received = batch.outcomes.len();
    let mut failures: Vec<kraken_core::Error> = Vec::new();
    for outcome in batch.outcomes {
        match outcome {
            Ok(result) => sink::print_result(fmt, P::METHOD, &result)?,
            Err(e) => failures.push(e),
        }
    }

    // An id-less whole-frame rejection answers every id at once: the per-id replies
    // it leaves unanswered were never owed, so they are neither "missing" nor part
    // of the expected count the summary reports.
    let frame_rejected = failures
        .iter()
        .any(|f| matches!(f, kraken_core::Error::FrameRejected(_)));
    let (expected, missing) = if frame_rejected {
        (received, 0)
    } else {
        (expected, expected.saturating_sub(received))
    };
    if failures.is_empty() && missing == 0 {
        return Ok(());
    }
    let succeeded = received - failures.len();
    Err(batch_error(
        P::METHOD,
        succeeded,
        expected,
        failures,
        missing,
        batch.stopped_short,
    ))
}

/// Reduce a partial or mixed multi-reply batch to a single error envelope.
///
/// The successful replies are already on stdout; this reports how many of the
/// expected replies failed or never arrived. A lone failure is returned
/// verbatim so a rate limit keeps its full guidance (suggestion, retryable,
/// docs_url); otherwise the failures are summarised under the most actionable
/// category (see [`representative_category`]) so the batch classifies the same
/// way a single reply would, rather than flattening to `api`.
///
/// Takes the wire-typed failures, not [`KrakenError`]s: the verbatim-or-summary
/// decision turns on [`kraken_core::Error::FrameRejected`], so converting
/// earlier would erase the very variant it reads. The failures cross into the
/// CLI's error vocabulary here, after that decision.
fn batch_error(
    method: &str,
    succeeded: usize,
    expected: usize,
    mut failures: Vec<kraken_core::Error>,
    missing: usize,
    stopped_short: Option<kraken_core::Error>,
) -> KrakenError {
    // A lone failure is returned verbatim only when nothing else needs reporting:
    // every other reply arrived (missing == 0), or the failure IS the whole batch —
    // the server refused the frame with one id-less reply (`FrameRejected`), so the
    // per-id replies counted as "missing" were never owed and must not bury the
    // precise rejection in a shortfall summary. A lone *correlated* rejection with
    // replies still missing takes the summary path instead: the missing ids' state
    // is unknown, and that must never go unreported.
    let whole_frame =
        succeeded == 0 && matches!(failures[..], [kraken_core::Error::FrameRejected(_)]);
    if failures.len() == 1
        && (missing == 0 || whole_frame)
        && let Some(failure) = failures.pop()
    {
        return failure.into();
    }

    // The stop reason is read as the wire-typed error: its un-prefixed Display goes
    // into the message (converting first would nest "WebSocket error:" twice) and
    // its typed verify-first property decides the envelope below.
    let verify_first = stopped_short
        .as_ref()
        .is_some_and(kraken_core::Error::is_verify_first);
    let failures: Vec<KrakenError> = failures.into_iter().map(KrakenError::from).collect();
    let category = representative_category(&failures);
    let mut message = format!("{method}: {succeeded} of {expected} succeeded");
    if !failures.is_empty() {
        message.push_str(&format!(
            "; {} failed ({})",
            failures.len(),
            failures.iter().map(ToString::to_string).join(", ")
        ));
    }
    if missing > 0 {
        message.push_str(&format!("; {missing} received no reply"));
        if let Some(reason) = &stopped_short {
            message.push_str(&format!(" ({reason})"));
        }
    }
    // The typed verify-first verdict survives any mix of per-id failures; only a
    // more actionable category (auth, rate limit) may lead the envelope instead,
    // and there the reason's prose still carries the check-your-orders instruction.
    if verify_first && !matches!(category, ErrorCategory::RateLimit | ErrorCategory::Auth) {
        return KrakenError::WebSocket {
            message,
            verify_first: true,
        };
    }
    KrakenError::Api { category, message }
}

/// The category to report for a mixed batch: the most actionable failure wins,
/// since an agent must react to a rate limit (back off) or an auth failure
/// (re-key) before it worries about a plain rejection. With no explicit
/// failures — only replies that never arrived — the batch timed out, so the
/// `websocket` category (what a WS timeout maps to) is reported.
fn representative_category(failures: &[KrakenError]) -> ErrorCategory {
    failures
        .iter()
        .map(KrakenError::category)
        .max_by_key(|category| match category {
            ErrorCategory::RateLimit => 2,
            ErrorCategory::Auth => 1,
            // Spelled out (not `_`) so a newly added category must be ranked here
            // instead of silently joining the un-actionable tier.
            ErrorCategory::Api
            | ErrorCategory::Network
            | ErrorCategory::Validation
            | ErrorCategory::Config
            | ErrorCategory::WebSocket
            | ErrorCategory::Io
            | ErrorCategory::Timeout
            | ErrorCategory::Parse => 0,
        })
        .unwrap_or(ErrorCategory::WebSocket)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_contract::ws::{MethodCommand, WsCommand};
    use std::num::NonZeroU64;

    /// Parse a `kraken ws ...` argv and return the parsed `Ws` variant's
    /// pieces.
    fn parse_ws(
        argv: &[&str],
    ) -> std::result::Result<(Option<NonZeroU64>, WsCommand), clap::Error> {
        use clap::Parser;
        let mut full = vec!["kraken", "ws"];
        full.extend_from_slice(argv);
        crate::Cli::try_parse_from(full).map(|cli| match cli.command {
            Some(crate::commands::Command::Streaming(
                kraken_contract::streaming::StreamingCommand::Ws(ws),
            )) => (ws.req_id, ws.cmd),
            _ => panic!("argv did not parse to a ws subcommand"),
        })
    }

    /// Parse a minimal `ws add-order` argv plus `extra` and return the parsed
    /// `validate` flag. Pins the `--validate` flag shape: a bare flag, matching
    /// the REST `order --validate`. The wire default is false, so a valued form
    /// (`--validate true`) would add no behaviour and must be rejected.
    fn add_order_validate_of(extra: &[&str]) -> std::result::Result<bool, clap::Error> {
        let mut argv = vec![
            "add-order",
            "--side",
            "buy",
            "--symbol",
            "BTC/USD",
            "--order-type",
            "limit",
        ];
        argv.extend_from_slice(extra);
        parse_ws(&argv).map(|(_, cmd)| match cmd {
            WsCommand::Method(MethodCommand::AddOrder(args)) => args.validate,
            _ => panic!("argv did not parse to ws add-order"),
        })
    }

    #[test]
    fn add_order_bare_validate_flag_parses_true() {
        assert!(add_order_validate_of(&["--validate"]).expect("bare flag must parse"));
    }

    #[test]
    fn add_order_absent_validate_flag_parses_false() {
        assert!(!add_order_validate_of(&[]).expect("absent flag must parse"));
    }

    #[test]
    fn add_order_valued_validate_flag_is_rejected() {
        assert!(add_order_validate_of(&["--validate", "true"]).is_err());
    }

    #[test]
    fn req_id_is_one_global_flag_on_every_subcommand() {
        // `--req-id` is declared once on `ws` and propagates to the subcommands, in
        // either position.
        let (req_id, _) = parse_ws(&["ping", "--req-id", "7"]).expect("global flag must parse");
        assert_eq!(req_id, NonZeroU64::new(7));
        let (req_id, _) = parse_ws(&["--req-id", "9", "ping"]).expect("pre-subcommand position");
        assert_eq!(req_id, NonZeroU64::new(9));
    }

    #[test]
    fn req_id_zero_is_a_parse_error() {
        // `NonZeroU64` makes 0 unrepresentable, so the reservation is enforced by clap
        // at the boundary instead of a runtime check.
        assert!(parse_ws(&["ping", "--req-id", "0"]).is_err());
    }

    fn api(message: &str) -> KrakenError {
        KrakenError::Api {
            category: ErrorCategory::Api,
            message: message.to_string(),
        }
    }

    #[test]
    fn representative_category_prefers_rate_limit() {
        let failures = vec![
            api("EOrder:Unknown order"),
            KrakenError::from_kraken_error("EOrder:Rate limit exceeded"),
            KrakenError::Auth("bad key".into()),
        ];
        assert_eq!(representative_category(&failures), ErrorCategory::RateLimit);
    }

    #[test]
    fn representative_category_prefers_auth_over_a_plain_rejection() {
        let failures = vec![
            api("EOrder:Unknown order"),
            KrakenError::Auth("bad key".into()),
        ];
        assert_eq!(representative_category(&failures), ErrorCategory::Auth);
    }

    #[test]
    fn representative_category_of_missing_only_is_websocket() {
        assert_eq!(representative_category(&[]), ErrorCategory::WebSocket);
    }

    /// A correlated (req_id-echoed) per-order rejection, as `into_result` types
    /// it.
    fn rejected(msg: &str) -> kraken_core::Error {
        kraken_core::Error::ServerRejected(msg.into())
    }

    #[test]
    fn batch_error_returns_a_lone_failure_verbatim() {
        // A single rate-limited id keeps the rich RateLimit variant (suggestion,
        // retryable, docs_url) rather than being flattened into a summary
        // string.
        let failures = vec![rejected("EOrder:Rate limit exceeded")];
        let err = batch_error("cancel_order", 2, 3, failures, 0, None);
        assert!(matches!(err, KrakenError::RateLimit { .. }));
    }

    #[test]
    fn batch_error_returns_a_whole_frame_rejection_verbatim_despite_missing_replies() {
        // A 3-id cancel whose entire frame is rejected gets ONE id-less error reply
        // (typed `FrameRejected` by `into_result`); the two "missing" per-id replies
        // were never owed, so the precise rejection must surface — not a "2 received
        // no reply" shortfall summary.
        let failures = vec![kraken_core::Error::FrameRejected(
            "EGeneral:Invalid arguments".into(),
        )];
        let err = batch_error("cancel_order", 0, 3, failures, 2, None);
        assert_eq!(err.category(), ErrorCategory::Api);
        assert_eq!(err.to_string(), "EGeneral:Invalid arguments");
    }

    #[test]
    fn batch_error_reports_missing_replies_alongside_a_correlated_lone_rejection() {
        // One req_id-correlated rejection plus a mid-batch disconnect is
        // count-identical to a whole-frame rejection (1 failure, 0 succeeded, 2
        // missing) — but here the missing ids were owed replies and their state
        // is unknown, so the shortfall must surface alongside the rejection,
        // never be swallowed by the verbatim shortcut.
        let failures = vec![rejected("EOrder:Unknown order")];
        let err = batch_error(
            "cancel_order",
            0,
            3,
            failures,
            2,
            Some(kraken_core::Error::Unacknowledged),
        );
        let message = err.to_string();
        assert!(message.contains("0 of 3 succeeded"), "got: {message}");
        assert!(message.contains("EOrder:Unknown order"), "got: {message}");
        assert!(message.contains("2 received no reply"), "got: {message}");
    }

    #[test]
    fn batch_error_summarises_mixed_outcomes_under_the_top_category() {
        let failures = vec![
            rejected("EOrder:Unknown order"),
            rejected("EOrder:Rate limit exceeded"),
        ];
        let err = batch_error(
            "cancel_order",
            1,
            4,
            failures,
            1,
            Some(kraken_core::Error::Unacknowledged),
        );
        // A rate limit outranks the plain rejection, so the batch routes on rate_limit
        // — not flattened to `api`.
        assert_eq!(err.category(), ErrorCategory::RateLimit);
        let message = err.to_string();
        assert!(message.contains("1 of 4 succeeded"), "got: {message}");
        assert!(message.contains("2 failed"), "got: {message}");
        assert!(message.contains("1 received no reply"), "got: {message}");
    }

    #[test]
    fn batch_error_reports_a_truncated_all_success_batch_with_the_shortfalls_verdict() {
        // Every reply that arrived succeeded but the batch stopped short: an error
        // naming the shortfall, never a silent exit 0 — and the typed stop
        // reason keeps its verify-first marker instead of a bare count-derived
        // category.
        let err = batch_error(
            "cancel_order",
            2,
            3,
            vec![],
            1,
            Some(kraken_core::Error::Unacknowledged),
        );
        assert_eq!(err.category(), ErrorCategory::WebSocket);
        assert!(matches!(
            err,
            KrakenError::WebSocket {
                verify_first: true,
                ..
            }
        ));
        let message = err.to_string();
        assert!(message.contains("2 of 3 succeeded"), "got: {message}");
        assert!(message.contains("1 received no reply"), "got: {message}");
    }

    #[test]
    fn batch_error_without_a_stop_reason_still_reports_the_shortfall() {
        // The defensive shape (a shortfall the client typed no reason for) must still
        // fail loud under the websocket category rather than silently succeed.
        let err = batch_error("cancel_order", 2, 3, vec![], 1, None);
        assert_eq!(err.category(), ErrorCategory::WebSocket);
        assert!(err.to_string().contains("1 received no reply"), "{err}");
    }
}
