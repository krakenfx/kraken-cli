use crate::error::{Error, Result};
use std::{fmt, num::NonZeroU16, str::FromStr};
use url::Url;

const PRODUCTION_AUTHORIZE_URL: &str = "https://www.kraken.com/oauth/authorize";
const PRODUCTION_API_URL: &str = "https://api.kraken.com";
pub const PRODUCTION_SIGN_IN_HOST: &str = "www.kraken.com";
const PRODUCTION_API_HOST: &str = "api.kraken.com";

/// Registered as `http://localhost:<port>/callback` redirect URIs. They are below
/// all OS ephemeral port ranges and not assigned by IANA.
const REGISTERED_PORTS: [u16; 3] = [27461, 27462, 27463];

/// Not a secret: it identifies the app but proves nothing, so an open-source
/// binary can contain it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientId(String);

impl ClientId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ClientId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.is_empty() || !s.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(Error::Invalid {
                what: "client ID",
                value: s.to_string(),
            });
        }
        Ok(Self(s.to_string()))
    }
}

impl fmt::Display for ClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug)]
pub struct Endpoints {
    authorize: Url,
    token: Url,
    fast_api_key: Url,
}

impl Endpoints {
    /// Accepts only https, or http to a loopback IP for local test servers. No
    /// token goes over the network without TLS. A code from the production
    /// sign-in page is redeemed at the production API only: it grants the
    /// user's real account, wherever it is sent.
    pub fn new(authorize: Url, api: Url) -> Result<Self> {
        require_secure(&authorize)?;
        require_secure(&api)?;
        require_bare(&authorize)?;
        if is_production_sign_in(&authorize) {
            if !is_https_origin(&authorize, PRODUCTION_SIGN_IN_HOST) {
                return Err(invalid_sign_in(&authorize));
            }
            if !is_https_origin(&api, PRODUCTION_API_HOST) {
                return Err(Error::ProductionCodeOffKraken(without_userinfo(&api)));
            }
        }
        // Relative paths under a trailing slash keep a path prefix of `api`,
        // as the Spot client does.
        let mut base = api.clone();
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let endpoint = |path: &str| {
            base.join(path)
                .map_err(|_| Error::InsecureEndpoint(without_userinfo(&api)))
        };
        Ok(Self {
            token: endpoint("oauth/token")?,
            fast_api_key: endpoint("oauth/fast-api-key")?,
            authorize,
        })
    }

    #[expect(
        clippy::expect_used,
        reason = "two constant https URLs, so parsing and validation cannot fail"
    )]
    pub fn production() -> Self {
        let parse = |url: &str| Url::parse(url).expect("constant URL parses");
        Self::new(parse(PRODUCTION_AUTHORIZE_URL), parse(PRODUCTION_API_URL))
            .expect("production endpoints are https")
    }

    pub fn authorize(&self) -> &Url {
        &self.authorize
    }

    /// The sign-in grants the user's real account, so the caller must verify
    /// TLS for every request.
    pub fn is_production(&self) -> bool {
        is_production_sign_in(&self.authorize)
    }

    pub(crate) fn token(&self) -> &Url {
        &self.token
    }

    pub(crate) fn fast_api_key(&self) -> &Url {
        &self.fast_api_key
    }
}

impl Default for Endpoints {
    fn default() -> Self {
        Self::production()
    }
}

/// By host, not origin: a production page on another port still signs in to
/// the real account. DNS resolves `www.kraken.com.` to the same page, so the
/// trailing dot cannot opt out of the pins; `is_https_origin` then refuses it.
fn is_production_sign_in(authorize: &Url) -> bool {
    authorize
        .host_str()
        .map(|host| host.strip_suffix('.').unwrap_or(host))
        == Some(PRODUCTION_SIGN_IN_HOST)
}

/// `https` on the default port. The url crate stores the default port as none.
fn is_https_origin(url: &Url, host: &str) -> bool {
    url.scheme() == "https" && url.host_str() == Some(host) && url.port().is_none()
}

/// The CLI appends every parameter itself, so a query of the override would
/// come first and could replace the client, the redirect or the challenge.
fn require_bare(authorize: &Url) -> Result<()> {
    let bare = authorize.query().is_none()
        && authorize.fragment().is_none()
        && authorize.username().is_empty()
        && authorize.password().is_none();
    if bare {
        Ok(())
    } else {
        Err(invalid_sign_in(authorize))
    }
}

fn invalid_sign_in(authorize: &Url) -> Error {
    Error::Invalid {
        what: "sign-in URL",
        value: without_userinfo(authorize),
    }
}

fn require_secure(url: &Url) -> Result<()> {
    let loopback = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    match url.scheme() {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        _ => Err(Error::InsecureEndpoint(without_userinfo(url))),
    }
}

/// A URL for an error message. Its userinfo can hold a password.
fn without_userinfo(url: &Url) -> String {
    let mut shown = url.clone();
    if shown
        .set_username("")
        .and_then(|()| shown.set_password(None))
        .is_err()
    {
        // Only a URL without a host refuses userinfo, and it carries none.
        return url.to_string();
    }
    shown.to_string()
}

/// Tried in order. Each port must be registered for the client, because Kraken
/// matches redirect URIs exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RedirectPorts(Vec<NonZeroU16>);

impl RedirectPorts {
    #[cfg(test)]
    pub(crate) fn new(ports: impl IntoIterator<Item = u16>) -> Result<Self> {
        let ports: Vec<_> = ports.into_iter().filter_map(NonZeroU16::new).collect();
        if ports.is_empty() {
            return Err(Error::Invalid {
                what: "redirect ports",
                value: "none".to_string(),
            });
        }
        Ok(Self(ports))
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = u16> + '_ {
        self.0.iter().map(|port| port.get())
    }
}

impl Default for RedirectPorts {
    fn default() -> Self {
        Self(
            REGISTERED_PORTS
                .into_iter()
                .filter_map(NonZeroU16::new)
                .collect(),
        )
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub(crate) client_id: ClientId,
    pub(crate) endpoints: Endpoints,
    pub(crate) redirect_ports: RedirectPorts,
}

impl Config {
    /// Uses the production endpoints and the registered redirect ports.
    pub fn new(client_id: ClientId) -> Self {
        Self {
            client_id,
            endpoints: Endpoints::default(),
            redirect_ports: RedirectPorts::default(),
        }
    }

    #[must_use]
    pub fn with_endpoints(self, endpoints: Endpoints) -> Self {
        Self { endpoints, ..self }
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn with_redirect_ports(self, redirect_ports: RedirectPorts) -> Self {
        Self {
            redirect_ports,
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_endpoints_resolve_against_the_api_host() {
        let endpoints = Endpoints::production();
        assert_eq!(
            endpoints.token().as_str(),
            "https://api.kraken.com/oauth/token"
        );
        assert_eq!(
            endpoints.fast_api_key().as_str(),
            "https://api.kraken.com/oauth/fast-api-key"
        );
    }

    #[test]
    fn a_path_prefix_of_the_api_url_is_kept() {
        let endpoints = Endpoints::new(
            Url::parse("https://sign-in.example/oauth/authorize").unwrap(),
            Url::parse("https://gw.example/kraken").unwrap(),
        )
        .unwrap();
        assert_eq!(
            endpoints.token().as_str(),
            "https://gw.example/kraken/oauth/token"
        );
    }

    #[test]
    fn a_sign_in_url_with_its_own_parameters_is_refused() {
        let api = Url::parse("https://api.kraken.com").unwrap();
        for authorize in [
            "https://www.kraken.com/oauth/authorize?client_id=x",
            "https://www.kraken.com/oauth/authorize#code_challenge=x",
            "https://user@www.kraken.com/oauth/authorize",
            "https://www.kraken.com:8443/oauth/authorize",
        ] {
            assert!(
                matches!(
                    Endpoints::new(Url::parse(authorize).unwrap(), api.clone()),
                    Err(Error::Invalid { .. })
                ),
                "{authorize}"
            );
        }
    }

    #[test]
    fn a_production_code_is_never_sent_off_kraken() {
        let production = |api: &str| {
            Endpoints::new(
                Url::parse(PRODUCTION_AUTHORIZE_URL).unwrap(),
                Url::parse(api).unwrap(),
            )
        };
        for api in [
            "https://gw.example",
            "https://api.kraken.com.evil.example",
            "https://api.kraken.com:8443",
            "http://127.0.0.1:9000",
        ] {
            assert!(
                matches!(production(api), Err(Error::ProductionCodeOffKraken(_))),
                "{api}"
            );
        }
        assert!(
            production("https://api.kraken.com/")
                .unwrap()
                .is_production()
        );
    }

    #[test]
    fn a_refused_url_never_prints_its_password() {
        let api = Url::parse(PRODUCTION_API_URL).unwrap();
        for (authorize, api) in [
            (
                "https://user:hunter2@www.kraken.com/oauth/authorize",
                api.clone(),
            ),
            (
                PRODUCTION_AUTHORIZE_URL,
                Url::parse("http://user:hunter2@gw.example").unwrap(),
            ),
        ] {
            let error = Endpoints::new(Url::parse(authorize).unwrap(), api).unwrap_err();
            assert!(!error.to_string().contains("hunter2"), "{error}");
        }
    }

    #[test]
    fn a_trailing_dot_cannot_take_the_production_sign_in_off_its_pins() {
        let authorize = Url::parse("https://www.kraken.com./oauth/authorize").unwrap();
        for api in ["https://gw.example", PRODUCTION_API_URL] {
            assert!(
                matches!(
                    Endpoints::new(authorize.clone(), Url::parse(api).unwrap()),
                    Err(Error::Invalid { .. })
                ),
                "{api}"
            );
        }
    }

    #[test]
    fn plain_http_is_refused_off_loopback() {
        let http = Url::parse("http://api.example.com").unwrap();
        let https = Url::parse("https://www.example.com").unwrap();
        assert!(matches!(
            Endpoints::new(https, http),
            Err(Error::InsecureEndpoint(_))
        ));
    }

    #[test]
    fn plain_http_is_allowed_on_a_loopback_ip_only() {
        let local = Url::parse("http://127.0.0.1:9000").unwrap();
        assert!(Endpoints::new(local.clone(), local).is_ok());

        let named = Url::parse("http://localhost:9000").unwrap();
        assert!(Endpoints::new(named.clone(), named).is_err());
    }

    #[test]
    fn a_client_id_rejects_anything_but_alphanumerics() {
        assert!(
            "0123456789abcdef0123456789abcdef"
                .parse::<ClientId>()
                .is_ok()
        );
        assert!("".parse::<ClientId>().is_err());
        assert!("abc def".parse::<ClientId>().is_err());
        assert!("abc&scope=x".parse::<ClientId>().is_err());
    }

    #[test]
    fn redirect_ports_reject_an_empty_or_zero_list() {
        assert!(RedirectPorts::new([]).is_err());
        assert!(RedirectPorts::new([0]).is_err());
        assert_eq!(
            RedirectPorts::new([0, 8080])
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            [8080]
        );
    }
}
