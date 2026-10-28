//! `-o table` lines for the one-shot method results.

use super::{Summarize, SummaryLine};
use kraken_core::{
    AddOrderResult, AmendOrderResult, CancelAllOrdersAfterResult, CancelOrderResult, CountResult,
    Pong,
};

impl Summarize for AddOrderResult {
    fn summary(&self) -> String {
        // A live reply carries `order_id`; a `validate`-mode reply carries `validation`
        // in its place; fall back to a placeholder if the reply has neither.
        let (key, value) = match self.order_id.as_deref() {
            Some(oid) if !oid.is_empty() => ("order_id", oid.to_string()),
            _ => match &self.validation {
                Some(v) => ("validation", v.clone()),
                None => ("order_id", "-".to_string()),
            },
        };
        SummaryLine::default()
            .field(key, value)
            .opt("cl_ord_id", self.cl_ord_id.as_deref())
            .opt("order_userref", self.order_userref)
            .warnings(self.warnings.as_deref())
            .build()
    }
}

impl Summarize for AmendOrderResult {
    fn summary(&self) -> String {
        SummaryLine::default()
            .field("amend_id", &self.amend_id)
            .opt("order_id", self.order_id.as_deref())
            .opt("cl_ord_id", self.cl_ord_id.as_deref())
            .warnings(self.warnings.as_deref())
            .build()
    }
}

impl Summarize for CancelOrderResult {
    fn summary(&self) -> String {
        SummaryLine::default()
            .field("order_id", self.order_id.as_deref().unwrap_or("-"))
            .opt("cl_ord_id", self.cl_ord_id.as_deref())
            .warnings(self.warnings.as_deref())
            .build()
    }
}

impl Summarize for CountResult {
    fn summary(&self) -> String {
        SummaryLine::default()
            .field("cancelled", self.count)
            .warnings(self.warnings.as_deref())
            .build()
    }
}

impl Summarize for CancelAllOrdersAfterResult {
    fn summary(&self) -> String {
        SummaryLine::default()
            .field("trigger", &self.trigger_time)
            .field("current", &self.current_time)
            .build()
    }
}

impl Summarize for Pong {
    fn summary(&self) -> String {
        "ping-pong".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(order_id: Option<&str>, validation: Option<&str>) -> AddOrderResult {
        AddOrderResult {
            order_id: order_id.map(Into::into),
            cl_ord_id: None,
            order_userref: None,
            validation: validation.map(Into::into),
            warnings: None,
        }
    }

    #[test]
    fn summary_falls_back_to_validation_when_order_id_missing_or_empty() {
        assert_eq!(result(None, Some("ok")).summary(), "validation:ok");
        assert_eq!(result(Some(""), Some("ok")).summary(), "validation:ok");
    }

    #[test]
    fn summary_surfaces_warning_count() {
        let mut r = result(Some("OXXX"), None);
        r.warnings = Some(vec!["reduced qty".into(), "late".into()]);
        assert_eq!(r.summary(), "order_id:OXXX warnings:2");
    }
}
