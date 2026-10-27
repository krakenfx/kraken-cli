//! Kraken Spot REST (`/0/public`, `/0/private`), modelled from Kraken's API
//! description.
//!
//! Every endpoint is a type implementing [`Endpoint`]: it is the request's
//! parameters, it names the response type, and it states whether a lost reply
//! may be resent. [`SpotRest::query`] sends public endpoints and
//! [`SpotAccount::sign`] private ones; sending one through the other does not
//! compile.

#[cfg(test)]
mod client_tests;
mod envelope;
mod sign;
mod vocabulary;

pub mod account;
pub mod earn;
pub mod funding;
pub mod market;
pub mod subaccount;
pub mod trading;
pub mod ws_token;

use crate::{
    Auth, RestError,
    transport::{BodyLog, Delivery, Transport},
};
use reqwest::header::CONTENT_TYPE;
use secrecy::{ExposeSecret, SecretString};
use serde::{Serialize, de::DeserializeOwned};
use std::sync::Arc;
pub use vocabulary::{
    AssetClass, LedgerType, OrderFlag, OrderNote, OrderStatus, OrderType, PositionStatus,
    RebaseMultiplier, TimeInForce, Trigger,
};

const DEFAULT_URL: &str = "https://api.kraken.com";

mod sealed {
    pub trait Sealed {}
}

/// An endpoint's request. Sealed: only this crate names a Kraken path.
pub trait Endpoint: Serialize + sealed::Sealed {
    /// The path under `/0/public/` or `/0/private/`.
    const PATH: &'static str;
    /// [`Public`] or [`Private`].
    type Access: Access;
    type Response: DeserializeOwned;

    /// Fails closed: every signed call is a POST, so the method cannot tell a
    /// write from a read, and a read that forgets to opt in only loses retries.
    fn delivery(&self) -> Delivery {
        Delivery::AtMostOnce
    }
}

/// Who may call an endpoint, and what its responses may reveal in a log.
pub trait Access: sealed::Sealed {
    #[doc(hidden)]
    const PREFIX: &'static str;
    #[doc(hidden)]
    const BODY_LOG: BodyLog;
}

/// Market data: unsigned GET, query-string parameters.
#[derive(Debug)]
pub enum Public {}

/// Account data and actions: signed POST, JSON body.
#[derive(Debug)]
pub enum Private {}

impl sealed::Sealed for Public {}
impl sealed::Sealed for Private {}

impl Access for Public {
    const PREFIX: &'static str = "/0/public/";
    const BODY_LOG: BodyLog = BodyLog::Full;
}

impl Access for Private {
    const PREFIX: &'static str = "/0/private/";
    const BODY_LOG: BodyLog = BodyLog::SizeOnly;
}

/// The Spot client for market data. `Clone` shares the connection pool.
#[derive(Debug, Clone)]
pub struct SpotRest {
    transport: Transport,
    base_url: String,
}

impl SpotRest {
    /// `base_url` overrides the production URL.
    #[must_use]
    pub fn new(transport: Transport, base_url: Option<String>) -> Self {
        Self {
            transport,
            base_url: base_url.unwrap_or_else(|| DEFAULT_URL.to_owned()),
        }
    }

    #[must_use]
    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// This client signed in to the account `auth` belongs to.
    #[must_use]
    pub fn authenticate(&self, auth: impl Into<Arc<Auth>>) -> SpotAccount {
        SpotAccount {
            rest: self.clone(),
            auth: auth.into(),
            otp: None,
        }
    }

    pub async fn query<E: Endpoint<Access = Public>>(
        &self,
        request: &E,
    ) -> Result<E::Response, RestError> {
        let url = format!("{}{}{}", self.base_url, Public::PREFIX, E::PATH);
        tracing::debug!(method = "GET", %url, "request");
        self.transport
            .send(
                request.delivery(),
                || Ok(self.transport.http().get(&url).query(request)),
                |reply| envelope::decode::<E>(&reply),
            )
            .await
    }
}

/// The Spot client signed in to one account, the only way to reach a private
/// endpoint. `Clone` shares the pool and the key.
#[derive(Debug, Clone)]
pub struct SpotAccount {
    rest: SpotRest,
    auth: Arc<Auth>,
    otp: Option<SecretString>,
}

impl SpotAccount {
    /// Replaces the second factor every request carries.
    #[must_use]
    pub fn with_otp(mut self, otp: Option<SecretString>) -> Self {
        self.otp = otp;
        self
    }

    #[must_use]
    pub fn auth(&self) -> &Arc<Auth> {
        &self.auth
    }

    #[must_use]
    pub fn otp(&self) -> Option<&SecretString> {
        self.otp.as_ref()
    }

    pub async fn sign<E: Endpoint<Access = Private>>(
        &self,
        request: &E,
    ) -> Result<E::Response, RestError> {
        self.post(request, |reply| envelope::decode::<E>(&reply))
            .await
    }

    /// The finished export's archive. Its success body is the file itself,
    /// not an envelope.
    pub async fn download(&self, request: &account::RetrieveExport) -> Result<Vec<u8>, RestError> {
        self.post(request, envelope::file).await
    }

    async fn post<E: Endpoint<Access = Private>, T>(
        &self,
        request: &E,
        decode: impl FnOnce(crate::transport::Reply) -> Result<T, RestError>,
    ) -> Result<T, RestError> {
        let path = format!("{}{}", Private::PREFIX, E::PATH);
        let url = format!("{}{path}", self.rest.base_url);
        let otp = self.otp.as_ref().map(ExposeSecret::expose_secret);
        let transport = &self.rest.transport;
        transport
            .send(
                request.delivery(),
                || {
                    let signed = sign::sign(&path, request, &self.auth, otp)?;
                    tracing::debug!(method = "POST", %url, nonce = signed.nonce, "request");
                    Ok(transport
                        .http()
                        .post(&url)
                        .header("API-Key", self.auth.key())
                        .header("API-Sign", signed.signature)
                        .header(CONTENT_TYPE, "application/json")
                        .body(signed.body))
                },
                decode,
            )
            .await
    }
}
