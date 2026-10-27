use crate::{
    Delivery,
    spot::{Endpoint, Public, sealed::Sealed},
};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `GET /0/public/SystemStatus`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SystemStatusQuery {}

/// The exchange's trading mode, with any announced maintenance or live
/// incident. The timestamps are RFC 3339 strings, published as sent.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemStatus {
    pub status: TradingStatus,
    pub timestamp: String,
    pub upcoming_maintenance: Option<Vec<Maintenance>>,
    pub emergency: Option<Vec<Emergency>>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Maintenance {
    pub event_id: Option<i64>,
    pub expected_start_utc: Option<String>,
    pub expected_end_utc: Option<String>,
    pub time_to_start_s: Option<i64>,
    pub phase: Option<MaintenancePhase>,
    pub affected_services: Option<Vec<Service>>,
    pub order_submission: Option<OrderSubmission>,
    pub recommended_action: Option<RecommendedAction>,
    pub cancel_before_utc: Option<String>,
    pub title: Option<String>,
    pub source_url: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Emergency {
    pub event_id: Option<i64>,
    pub incident_status: Option<IncidentStatus>,
    pub impact: Option<Impact>,
    pub affected_services: Option<Vec<Service>>,
    pub started_at_utc: Option<String>,
    pub next_steps: Option<Vec<NextStep>>,
    pub source_url: Option<String>,
    pub title: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextStep {
    pub applies_to: Option<Vec<Service>>,
    #[serde(rename = "type")]
    pub kind: Option<NextStepKind>,
    pub expected_at_utc: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TradingStatus {
    Online,
    CancelOnly,
    PostOnly,
    LimitOnly,
    Maintenance,
    #[serde(untagged, deserialize_with = "crate::primitives::unrecognized")]
    Unrecognized(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenancePhase {
    Announced,
    #[serde(rename = "reminder_24h")]
    Reminder24h,
    #[serde(rename = "approaching_30m")]
    Approaching30m,
    #[serde(rename = "imminent_5m")]
    Imminent5m,
    #[serde(rename = "final_warning_30s")]
    FinalWarning30s,
    #[serde(untagged, deserialize_with = "crate::primitives::unrecognized")]
    Unrecognized(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Service {
    SpotWs,
    SpotRest,
    SpotFix,
    SpotTrading,
    FuturesWs,
    FuturesRest,
    FuturesFix,
    FuturesTrading,
    All,
    #[serde(untagged, deserialize_with = "crate::primitives::unrecognized")]
    Unrecognized(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderSubmission {
    Allowed,
    Discouraged,
    Blocked,
    #[serde(untagged, deserialize_with = "crate::primitives::unrecognized")]
    Unrecognized(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendedAction {
    Continue,
    ReduceActivity,
    CancelOpenOrders,
    StopNewOrders,
    DisconnectAndWait,
    #[serde(untagged, deserialize_with = "crate::primitives::unrecognized")]
    Unrecognized(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentStatus {
    Investigating,
    Identified,
    Monitoring,
    #[serde(untagged, deserialize_with = "crate::primitives::unrecognized")]
    Unrecognized(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Impact {
    None,
    Minor,
    Major,
    Critical,
    #[serde(untagged, deserialize_with = "crate::primitives::unrecognized")]
    Unrecognized(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NextStepKind {
    ExpectedRestart,
    ExpectedCancelOnly,
    ExpectedPostOnly,
    ExpectedOnline,
    #[serde(untagged, deserialize_with = "crate::primitives::unrecognized")]
    Unrecognized(String),
}

impl Sealed for SystemStatusQuery {}
impl Endpoint for SystemStatusQuery {
    const PATH: &'static str = "SystemStatus";
    type Access = Public;
    type Response = SystemStatus;

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
        let status: SystemStatus = decode_fixture("SystemStatus.json").unwrap();
        assert_eq!(status.status, TradingStatus::Online);
    }

    #[test]
    fn a_new_trading_mode_passes_through_verbatim() {
        let status: SystemStatus =
            serde_json::from_str(r#"{"status":"reduce_only","timestamp":"t"}"#).unwrap();
        assert_eq!(
            status.status,
            TradingStatus::Unrecognized("reduce_only".into())
        );
        assert_eq!(
            serde_json::to_string(&status).unwrap(),
            r#"{"status":"reduce_only","timestamp":"t"}"#
        );
    }
}
