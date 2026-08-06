//! Isolated account scopes for paper and live trading.
//!
//! Journals, sessions, and experiments are workspace-local; recorded market
//! data remains global.

mod app;
mod manifest;
mod naming;
pub mod policy;
pub mod promote;
pub mod receipt;
pub mod report;
pub mod session;
mod store;

use std::path::{Path, PathBuf};

pub use app::{
    AssetBalance, CreateSpec, ListedWorkspace, ResetOverrides, Target, WorkspaceBalances,
    WorkspaceOverview, Workspaces, journal_balances,
};
pub use manifest::{
    DECISIONS_FILE, JOURNAL_FILE, WORKSPACE_FILE, WORKSPACE_VERSION, WorkspaceManifest,
    WorkspaceMode, is_compatible,
};
pub use naming::{DEFAULT_WORKSPACE, WorkspaceName};
pub use store::{journal_path, manifest_path};

pub type Result<T, E = WorkspaceError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    /// The name has no home under `workspaces/` — scoping into it would
    /// silently create a parallel empty tree. → envelope `validation`.
    #[error(
        "Workspace '{0}' not found. Create it with 'kraken workspace create {0} --capital <amount> --mode paper'."
    )]
    NotFound(String),
    /// A create under a name that already holds an account. → envelope `validation`.
    #[error("Workspace '{0}' already exists.")]
    Exists(String),
    /// Bad name or spec input. → envelope `validation`.
    #[error("{0}")]
    Spec(String),
    /// A balance-setting operation on an account whose capital is not locally
    /// managed (the real account, or a non-paper mode). → envelope `validation`.
    #[error("{0}")]
    CapitalManaged(String),
    /// Refuses named live workspaces until credentials can be scoped safely.
    #[error(
        "Workspace '{name}' cannot go live yet: live execution needs workspace-scoped \
         credentials. Create it with --mode paper and promote it later."
    )]
    LiveUnavailable { name: String },
    /// Refuses pairs outside the workspace allow-list.
    #[error("pair {pair} is not permitted in workspace '{workspace}' (allowed: {allowed})")]
    PairNotAllowed {
        pair: String,
        workspace: String,
        allowed: String,
    },
    /// A money surface with no paper equivalent, refused inside a paper
    /// workspace rather than silently degraded. → envelope `validation`.
    #[error(
        "{what} has no paper equivalent; workspace '{workspace}' is mode {mode}. Promote the \
         workspace (kraken workspace promote) or unset KRAKEN_WORKSPACE to use the real account."
    )]
    Unsupported {
        what: String,
        workspace: String,
        mode: WorkspaceMode,
    },
    /// Futures paper state is still global (one snapshot for every scope) —
    /// mutating it from inside a workspace would corrupt shared state.
    /// → envelope `validation`.
    #[error("futures paper is not workspace-scoped yet; run it without a workspace")]
    FuturesUnscoped { workspace: String },
    /// A second `run start` (or a `workspace reset`) while a session is still
    /// recording. → envelope `validation`.
    #[error(
        "session {session} is still recording in this scope; stop it first: kraken session stop"
    )]
    SessionActive { session: String },
    /// A run-addressed command with nothing to address. → envelope `validation`.
    #[error("no active session in this scope; start one with 'kraken session start'")]
    NoActiveSession,
    /// A session ref that resolves to nothing; names what exists. → envelope
    /// `validation`.
    #[error("session '{reference}' not found (sessions: {known})")]
    SessionNotFound { reference: String, known: String },
    /// A `--label` another run already carries — labels resolve like
    /// ordinals, so they must be unique. → envelope `validation`.
    #[error("session label '{label}' is already taken by {session}")]
    LabelTaken { label: String, session: String },
    /// A promotion evaluated but refused — the checklist rides the error so
    /// the caller (and the JSON envelope) can say exactly what is missing.
    /// → envelope `validation`, with a `checklist` field.
    #[error("workspace '{workspace}' is not promotable yet; the checklist names what is missing")]
    NotPromotable {
        workspace: String,
        checklist: Box<promote::PromotionChecklist>,
    },
    /// Run artifact failure (session.json, windowed read); the envelope mapping
    /// delegates to the session bridge.
    #[error(transparent)]
    Session(#[from] kraken_session::SessionError),
    /// A contract written by a newer MAJOR layout. → envelope `config`.
    #[error(
        "workspace contract version {found} is incompatible with this build (expects {expected})"
    )]
    Incompatible { found: String, expected: String },
    /// A stored contract this build cannot trust: undecodable, or naming a
    /// different workspace than its directory. → envelope `parse`.
    #[error("damaged workspace '{name}': {what}")]
    Damaged { name: String, what: String },
    /// The account journal refused an operation; the envelope mapping
    /// delegates to the paper bridge.
    #[error(transparent)]
    Paper(#[from] kraken_paper::PaperError),
    /// Durable storage under the workspace failed; delegates to the
    /// recording bridge.
    #[error(transparent)]
    Recording(#[from] kraken_recording::Error),
    #[error(transparent)]
    Store(#[from] std::io::Error),
}

impl WorkspaceError {
    /// Stable category mirrored by the CLI bridge and pinned by cross-layer tests.
    pub fn category(&self) -> &'static str {
        match self {
            Self::NotFound(_)
            | Self::Exists(_)
            | Self::Spec(_)
            | Self::CapitalManaged(_)
            | Self::LiveUnavailable { .. }
            | Self::PairNotAllowed { .. }
            | Self::Unsupported { .. }
            | Self::FuturesUnscoped { .. }
            | Self::SessionActive { .. }
            | Self::NoActiveSession
            | Self::SessionNotFound { .. }
            | Self::LabelTaken { .. }
            | Self::NotPromotable { .. } => "validation",
            Self::Incompatible { .. } => "config",
            Self::Damaged { .. } => "parse",
            Self::Paper(err) => paper_category(err),
            Self::Recording(err) => recording_category(err),
            Self::Session(err) => session_category(err),
            Self::Store(_) => "io",
        }
    }
}

/// Mirrors the CLI bridge so crate-level errors retain stable categories.
fn session_category(err: &kraken_session::SessionError) -> &'static str {
    match err {
        kraken_session::SessionError::Rejected(_) => "validation",
        kraken_session::SessionError::IncompatibleManifest { .. } => "config",
        kraken_session::SessionError::Damaged(_) => "parse",
        kraken_session::SessionError::Recording(inner) => recording_category(inner),
        kraken_session::SessionError::Io(_) => "io",
    }
}

/// Mirror of the binary's `From<PaperError>` bridge, for self-labeling.
fn paper_category(err: &kraken_paper::PaperError) -> &'static str {
    match err {
        kraken_paper::PaperError::Rejected(_) => "validation",
        kraken_paper::PaperError::Incompatible(_) => "config",
        kraken_paper::PaperError::Journal(inner) => recording_category(inner),
    }
}

/// Mirror of the binary's `From<kraken_recording::Error>` bridge.
fn recording_category(err: &kraken_recording::Error) -> &'static str {
    match err {
        kraken_recording::Error::Rejected(_) => "validation",
        kraken_recording::Error::Damaged(_) => "parse",
        // Io and (with the duckdb feature) Engine are both local-I/O failures.
        _ => "io",
    }
}

/// The root under which all named workspaces live: `<base>/workspaces/`.
/// `base` is the config dir in production and a temp dir in tests.
pub fn workspaces_root(base: &Path) -> PathBuf {
    base.join("workspaces")
}

/// A workspace's scope root: `<base>/workspaces/<name>/`.
pub fn workspace_dir(base: &Path, name: &WorkspaceName) -> PathBuf {
    workspaces_root(base).join(name.as_ref())
}

/// Resolves scope paths without I/O; absent and `default` preserve the flat layout.
pub fn scoped_base(base: &Path, workspace: Option<&WorkspaceName>) -> PathBuf {
    match workspace {
        None => base.to_path_buf(),
        Some(name) if name.is_default() => base.to_path_buf(),
        Some(name) => workspace_dir(base, name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_base_aliases_none_and_default_to_the_flat_layout() {
        let base = Path::new("/cfg");
        let default: WorkspaceName = DEFAULT_WORKSPACE.parse().expect("reserved name is valid");
        assert_eq!(scoped_base(base, None), base);
        assert_eq!(scoped_base(base, Some(&default)), base);
    }

    #[test]
    fn scoped_base_roots_a_named_workspace_one_level_under_workspaces() {
        let base = Path::new("/cfg");
        let name: WorkspaceName = "btc-momentum".parse().expect("valid name");
        assert_eq!(
            scoped_base(base, Some(&name)),
            Path::new("/cfg/workspaces/btc-momentum")
        );
    }

    #[test]
    fn error_categories_self_label_for_every_variant() {
        assert_eq!(
            WorkspaceError::NotFound("x".into()).category(),
            "validation"
        );
        assert_eq!(WorkspaceError::Exists("x".into()).category(), "validation");
        assert_eq!(WorkspaceError::Spec("x".into()).category(), "validation");
        assert_eq!(
            WorkspaceError::CapitalManaged("x".into()).category(),
            "validation"
        );
        assert_eq!(
            WorkspaceError::LiveUnavailable { name: "x".into() }.category(),
            "validation"
        );
        assert_eq!(
            WorkspaceError::Incompatible {
                found: "2.0".into(),
                expected: "1.0".into()
            }
            .category(),
            "config"
        );
        assert_eq!(
            WorkspaceError::Damaged {
                name: "x".into(),
                what: "y".into()
            }
            .category(),
            "parse"
        );
    }
}
