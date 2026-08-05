//! The snapshot/update discriminant every channel-data frame carries.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

/// Whether a frame carries a full snapshot or an incremental update.
///
/// `Display` and `FromStr` speak the same lowercase wire form as serde (matching
/// [`OrderSide`](crate::OrderSide)/[`OrderType`](crate::OrderType)), so a consumer that
/// stores the rendered tag can parse it back without a second vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum MessageType {
    Snapshot,
    Update,
}
