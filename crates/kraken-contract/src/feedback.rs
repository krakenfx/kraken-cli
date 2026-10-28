//! `kraken feedback`: capture product feedback from agents and users, upload

use clap::ValueEnum;
use std::path::PathBuf;

/// Which kind of report a feedback submission carries.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum FeedbackType {
    /// Request a new capability or improvement.
    #[value(name = "feature-request")]
    FeatureRequest,
    /// Report awkward or confusing CLI/skill behaviour.
    Friction,
    /// Report incorrect or broken behaviour.
    Bug,
}

impl FeedbackType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FeatureRequest => "feature-request",
            Self::Friction => "friction",
            Self::Bug => "bug",
        }
    }
}

/// Submit product feedback (feature request, friction, or bug).
#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Feedback {
    /// Feedback category: feature-request, friction, or bug.
    #[arg(value_enum)]
    pub feedback_type: FeedbackType,

    /// Inline feedback body (mutually exclusive with --input-file).
    #[arg(
        long,
        required_unless_present = "input_file",
        conflicts_with = "input_file"
    )]
    pub input_string: Option<String>,

    /// Read the feedback body from a UTF-8 file (mutually exclusive with
    /// --input-string).
    #[arg(
        long,
        required_unless_present = "input_string",
        conflicts_with = "input_string"
    )]
    pub input_file: Option<PathBuf>,

    /// Optional LLM / agent model identity (e.g. claude-opus-4, gpt-5).
    #[arg(long)]
    pub llm_model: Option<String>,
}

pub const LONG_ABOUT: &str = "\
Submit product feedback so we can continuously improve kraken-cli and adapt it to what users and \
                              agents need.

Submitting uploads the feedback text (and metadata such as CLI version, instance id, and optional \
                              --llm-model) to Kraken over HTTPS. Do not include API keys, \
                              secrets, passwords, or personal data in the body.

Types (soft recommendations for the body; only a string is required):
  feature-request  Desired outcome, why it matters, and any current workaround.
  friction         What you tried, what felt wrong, and expected vs actual.
  bug              Description, steps to reproduce, and versions (CLI via `kraken --version`, OS, \
                              skill name/version if relevant). Prefer --llm-model for the model \
                              identity instead of burying it only in the body.

Examples:
  kraken feedback bug --input-string \"order buy rejected with unclear error\" -o json
  kraken feedback friction --input-file ./notes.txt --llm-model claude-opus-4 -o json

The body must be non-empty after trim and at most 8192 UTF-8 bytes. A failed upload fails the \
                              command.
";
