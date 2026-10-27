use crate::{
    Delivery,
    spot::{Endpoint, Public, sealed::Sealed},
};
use serde::{Deserialize, Serialize};

/// `GET /0/public/Time`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ServerTimeQuery {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerTime {
    pub unixtime: i64,
    pub rfc1123: String,
}

impl Sealed for ServerTimeQuery {}
impl Endpoint for ServerTimeQuery {
    const PATH: &'static str = "Time";
    type Access = Public;
    type Response = ServerTime;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;

    #[test]
    fn decodes_the_captured_reply() {
        let time: ServerTime = decode_fixture("Time.json").unwrap();
        assert_eq!(time.unixtime, 1792499897);
    }
}
