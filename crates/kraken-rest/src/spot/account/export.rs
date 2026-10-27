//! The export lifecycle: request a report, poll its status, download it,
//! remove it.

use crate::{
    Delivery,
    spot::{Endpoint, Private, sealed::Sealed},
};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportReportKind {
    Trades,
    Ledgers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ExportFormat {
    Csv,
    Tsv,
}

/// `POST /0/private/AddExport`. Queues a new report on every call, so a lost
/// reply is never resent.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct AddExport {
    pub report: ExportReportKind,
    pub description: String,
    pub format: ExportFormat,
    /// Comma-separated columns; all when absent.
    pub fields: Option<String>,
    /// Unix seconds.
    pub starttm: Option<i64>,
    pub endtm: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportCreated {
    pub id: String,
}

/// `POST /0/private/ExportStatus`.
#[derive(Debug, Clone, Serialize)]
pub struct ExportStatusQuery {
    pub report: ExportReportKind,
}

pub type ExportStatus = Vec<ExportReport>;

/// One report's state. Published as sent: every time is a Unix-seconds string.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportReport {
    pub id: String,
    pub descr: String,
    pub format: String,
    pub report: String,
    pub subtype: Option<String>,
    pub status: ExportProgress,
    pub error: Option<String>,
    pub flags: Option<String>,
    pub fields: Option<String>,
    pub createdtm: Option<String>,
    pub expiretm: Option<String>,
    pub starttm: Option<String>,
    pub completedtm: Option<String>,
    pub datastarttm: Option<String>,
    pub dataendtm: Option<String>,
    pub aclass: Option<String>,
    pub asset: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportProgress {
    Queued,
    Processing,
    Processed,
}

/// `POST /0/private/RetrieveExport`. The reply is the report's zip archive.
#[derive(Debug, Clone, Serialize)]
pub struct RetrieveExport {
    pub id: String,
}

/// `POST /0/private/RemoveExport`.
#[derive(Debug, Clone, Serialize)]
pub struct RemoveExport {
    pub id: String,
    #[serde(rename = "type")]
    pub removal: Removal,
}

/// `Cancel` stops a queued or processing report; `Delete` drops a finished one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Removal {
    Cancel,
    Delete,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportRemoved {
    pub delete: Option<bool>,
    pub cancel: Option<bool>,
}

impl Sealed for AddExport {}
impl Endpoint for AddExport {
    const PATH: &'static str = "AddExport";
    type Access = Private;
    type Response = ExportCreated;
}

impl Sealed for ExportStatusQuery {}
impl Endpoint for ExportStatusQuery {
    const PATH: &'static str = "ExportStatus";
    type Access = Private;
    type Response = ExportStatus;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

impl Sealed for RetrieveExport {}
impl Endpoint for RetrieveExport {
    const PATH: &'static str = "RetrieveExport";
    type Access = Private;
    /// Never decoded:
    /// [`SpotAccount::download`](crate::spot::SpotAccount::download)
    /// returns the archive's bytes.
    type Response = ();

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

impl Sealed for RemoveExport {}
impl Endpoint for RemoveExport {
    const PATH: &'static str = "RemoveExport";
    type Access = Private;
    type Response = ExportRemoved;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;

    #[test]
    fn decodes_the_report_list() {
        let reports: ExportStatus = decode_fixture("ExportStatus.json").unwrap();
        assert!(!reports.is_empty());
    }

    #[test]
    fn decodes_the_created_id() {
        let created: ExportCreated = decode_fixture("AddExport.json").unwrap();
        assert!(!created.id.is_empty());
    }

    #[test]
    fn decodes_the_removal() {
        let removed: ExportRemoved = decode_fixture("RemoveExport.json").unwrap();
        assert!(removed.delete.is_some() || removed.cancel.is_some());
    }
}
