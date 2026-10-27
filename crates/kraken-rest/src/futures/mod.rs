//! Kraken Futures REST: the trading API (`/derivatives/api/v3`) and the
//! history API (`/api/history/v3`), modelled from Kraken's API descriptions
//! (<https://docs.kraken.com/openapi/futures-rest.yaml>,
//! <https://docs.kraken.com/openapi/futures-history-rest.yaml>).
//!
//! Every reply publishes as Kraken sent it, `result` and `serverTime` included.

#[cfg(test)]
mod client_tests;
mod envelope;
mod form;
mod sign;
mod vocabulary;

pub mod account;
pub mod history;
pub mod market;
pub mod trading;
pub mod transfers;

use crate::{
    Auth, Delivery, RestError,
    transport::{BodyLog, Reply as HttpReply, Transport},
};
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_with::skip_serializing_none;
pub use sign::sign_challenge;
use std::{borrow::Cow, fmt, sync::Arc};
pub use vocabulary::*;

const DEFAULT_URL: &str = "https://futures.kraken.com/derivatives/api/v3";
const TRADING_SUFFIX: &str = "/derivatives/api/v3";
const HISTORY_SUFFIX: &str = "/api/history/v3";

mod sealed {
    pub trait Sealed {}
}

/// An endpoint's request. Sealed: only this crate names a Kraken path.
pub trait Endpoint: Serialize + sealed::Sealed {
    /// The path under its API's root.
    const PATH: &'static str;
    const METHOD: Method;
    const API: Api = Api::Trading;
    /// [`Public`] or [`Private`].
    type Access: Access;
    type Response: DeserializeOwned;

    /// The path with any parameters filled in (`tickers/PF_XBTUSD`).
    fn path(&self) -> Cow<'static, str> {
        Cow::Borrowed(Self::PATH)
    }

    /// Reads and PUT setters are resent after a lost reply: setting a value
    /// twice leaves it set. A POST acts, so it never is.
    fn delivery(&self) -> Delivery {
        match Self::METHOD {
            Method::Get | Method::Put => Delivery::Retryable,
            Method::Post => Delivery::AtMostOnce,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
}

/// Which of the two Futures APIs serves an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    Trading,
    History,
}

impl Api {
    /// The path Kraken signs: the API's own prefix, without `/derivatives`.
    fn signed_prefix(self) -> &'static str {
        match self {
            Self::Trading => "/api/v3/",
            Self::History => "/api/history/v3/",
        }
    }
}

/// Who may call an endpoint, and what its responses may reveal in a log.
pub trait Access: sealed::Sealed {
    #[doc(hidden)]
    const BODY_LOG: BodyLog;
}

/// Market data: unsigned.
#[derive(Debug)]
pub enum Public {}

/// Account data and actions: `APIKey`/`Nonce`/`Authent` headers.
#[derive(Debug)]
pub enum Private {}

impl sealed::Sealed for Public {}
impl sealed::Sealed for Private {}

impl Access for Public {
    const BODY_LOG: BodyLog = BodyLog::Full;
}

impl Access for Private {
    const BODY_LOG: BodyLog = BodyLog::SizeOnly;
}

/// Every Futures reply: the outcome, the endpoint's own fields, and the
/// server's clock, in Kraken's order.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply<T> {
    pub result: Option<Outcome>,
    #[serde(flatten)]
    pub body: T,
    #[serde(rename = "serverTime")]
    pub server_time: Option<String>,
}

/// A rejection never decodes as a reply; it surfaces as
/// [`RestError::Rejected`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Success,
}

/// A value placed into a request path: a symbol or an account id. Anything
/// that could reach another path is refused.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PathSegment(String);

/// Why a value cannot sit in a request path.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{what} {value:?} {reason}")]
pub struct InvalidSegment {
    what: &'static str,
    value: String,
    reason: &'static str,
}

impl PathSegment {
    /// `what` names the value in the error (`symbol`, `subaccount_uid`).
    pub fn new(what: &'static str, value: impl Into<String>) -> Result<Self, InvalidSegment> {
        let value = value.into();
        let reason = if value.is_empty() {
            Some("cannot be empty")
        } else if value.contains('/') {
            Some("must not contain '/'")
        } else if value.contains("..") {
            Some("must not contain '..'")
        } else if value.contains(char::is_whitespace) {
            Some("must not contain whitespace")
        } else if value.chars().any(|c| matches!(c, '?' | '#' | '%' | '\\')) {
            Some("must not contain reserved URL characters (?, #, %, \\)")
        } else {
            None
        };
        match reason {
            Some(reason) => Err(InvalidSegment {
                what,
                value,
                reason,
            }),
            None => Ok(Self(value)),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PathSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The Futures client for market data. `Clone` shares the connection pool.
#[derive(Debug, Clone)]
pub struct FuturesRest {
    transport: Transport,
    trading_root: String,
    history_root: String,
}

impl FuturesRest {
    /// `base_url` overrides the trading API root; the history API's root is
    /// derived from it.
    #[must_use]
    pub fn new(transport: Transport, base_url: Option<String>) -> Self {
        let trading_root = base_url.unwrap_or_else(|| DEFAULT_URL.to_owned());
        let trading_root = trading_root.trim_end_matches('/').to_owned();
        let history_root = match trading_root.strip_suffix(TRADING_SUFFIX) {
            Some(host) => format!("{host}{HISTORY_SUFFIX}"),
            // A local venue serves both APIs under one root.
            None => format!("{trading_root}{HISTORY_SUFFIX}"),
        };
        Self {
            transport,
            trading_root,
            history_root,
        }
    }

    /// The trading API root, the URL this client was built from.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.trading_root
    }

    /// This client signed in to the account `auth` belongs to.
    #[must_use]
    pub fn authenticate(&self, auth: impl Into<Arc<Auth>>) -> FuturesAccount {
        FuturesAccount {
            rest: self.clone(),
            auth: auth.into(),
        }
    }

    pub async fn query<E: Endpoint<Access = Public>>(
        &self,
        request: &E,
    ) -> Result<E::Response, RestError> {
        self.send(request, None, |reply| envelope::decode::<E>(&reply))
            .await
    }

    async fn send<E: Endpoint, T>(
        &self,
        request: &E,
        auth: Option<&Auth>,
        decode: impl FnOnce(HttpReply) -> Result<T, RestError>,
    ) -> Result<T, RestError> {
        let root = match E::API {
            Api::Trading => &self.trading_root,
            Api::History => &self.history_root,
        };
        let path = request.path();
        let params = form::encode(request)?;
        let url = match (E::METHOD, params.is_empty()) {
            (Method::Get, false) => format!("{root}/{path}?{params}"),
            _ => format!("{root}/{path}"),
        };
        let signed_path = format!("{}{path}", E::API.signed_prefix());
        tracing::debug!(method = ?E::METHOD, %url, authenticated = auth.is_some(), "request");
        self.transport
            .send(
                request.delivery(),
                || {
                    let http = self.transport.http();
                    let builder = match E::METHOD {
                        Method::Get => http.get(&url),
                        Method::Post => http.post(&url),
                        Method::Put => http.put(&url),
                    };
                    let builder = match E::METHOD {
                        Method::Get => builder,
                        Method::Post | Method::Put => builder
                            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                            .body(params.clone()),
                    };
                    match auth {
                        None => Ok(builder),
                        Some(auth) => {
                            let headers = sign::headers(&params, &signed_path, auth)?;
                            Ok(builder
                                .header("APIKey", auth.key())
                                .header("Nonce", headers.nonce)
                                .header("Authent", headers.authent))
                        }
                    }
                },
                decode,
            )
            .await
    }
}

/// The Futures client signed in to one account, the only way to reach a
/// private endpoint. `Clone` shares the pool and the key.
#[derive(Debug, Clone)]
pub struct FuturesAccount {
    rest: FuturesRest,
    auth: Arc<Auth>,
}

impl FuturesAccount {
    #[must_use]
    pub fn auth(&self) -> &Arc<Auth> {
        &self.auth
    }

    pub async fn sign<E: Endpoint<Access = Private>>(
        &self,
        request: &E,
    ) -> Result<E::Response, RestError> {
        self.rest
            .send(request, Some(&self.auth), |reply| {
                envelope::decode::<E>(&reply)
            })
            .await
    }

    /// The account log as CSV text. Its success body is the file, not a reply.
    pub async fn download(&self, request: &history::AccountLogCsv) -> Result<String, RestError> {
        self.rest
            .send(
                request,
                Some(&self.auth),
                envelope::text::<history::AccountLogCsv>,
            )
            .await
    }
}
