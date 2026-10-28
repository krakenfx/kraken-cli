//! Futures parameters as one `application/x-www-form-urlencoded` string: the
//! query of a GET, the body of a POST or PUT, and in both cases the `postData`
//! Kraken signs, byte for byte.

use crate::RestError;
use serde::Serialize;
use serde_json::Value;

/// Requests are flat structs of scalars; a nested value (a batch) is sent as
/// its JSON text.
pub(super) fn encode(request: &impl Serialize) -> Result<String, RestError> {
    let fields = match serde_json::to_value(request) {
        Ok(Value::Object(fields)) => fields,
        Ok(Value::Null) => return Ok(String::new()),
        Ok(_) => {
            return Err(RestError::Encode(
                "a request must serialize as an object".into(),
            ));
        }
        Err(e) => {
            return Err(RestError::Encode(format!(
                "Failed to serialize request: {e}"
            )));
        }
    };
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in fields {
        let text = match value {
            Value::Null => continue,
            Value::String(text) => text,
            nested @ (Value::Array(_) | Value::Object(_)) => nested.to_string(),
            scalar @ (Value::Bool(_) | Value::Number(_)) => scalar.to_string(),
        };
        form.append_pair(&key, &text);
    }
    Ok(form.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Order {
        symbol: &'static str,
        size: u32,
        #[serde(rename = "reduceOnly")]
        reduce_only: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        stop: Option<u32>,
    }

    #[test]
    fn scalars_encode_in_declaration_order() {
        let order = Order {
            symbol: "PF_XBTUSD",
            size: 1,
            reduce_only: true,
            stop: None,
        };
        assert_eq!(
            encode(&order).unwrap(),
            "symbol=PF_XBTUSD&size=1&reduceOnly=true"
        );
    }

    #[test]
    fn an_empty_request_is_no_parameters() {
        #[derive(Serialize)]
        struct Empty {}
        assert_eq!(encode(&Empty {}).unwrap(), "");
    }
}
