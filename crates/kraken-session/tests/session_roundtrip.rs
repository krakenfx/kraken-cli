//! One realistic session flow through the public API only — the boundary
//! proof for every timeline consumer: provision, write all three tracks with
//! the public writers, read the merged timeline, and pin the merge order,
//! the writer-lock refusal, and the unknown-kind skip.

use kraken_session::timeline::event::Track;
use kraken_session::timeline::{self, fixtures};
use kraken_session::{SessionError, decisions_path, events_path, manifest_path};

#[test]
fn three_tracks_merge_in_at_track_seq_order() {
    let base = tempfile::tempdir().unwrap();
    let name = fixtures::provision(base.path(), vec![fixtures::jsonl_recording()]);
    fixtures::write_market(
        base.path(),
        &name,
        &["2026-01-01T00:00:01.000000Z", "2026-01-01T00:00:03.000000Z"],
    );
    fixtures::write_account(
        base.path(),
        &name,
        &[fixtures::account_record("2026-01-01T00:00:02+00:00")],
    );
    fixtures::write_decision(base.path(), &name, "2026-01-01T00:00:03+00:00");

    // The manifest round-trips through its own save/load on the way in
    // (fixtures::provision saved it; read loads it).
    let manifest =
        kraken_session::manifest::SessionManifest::load(&manifest_path(base.path(), &name))
            .unwrap();
    assert_eq!(manifest.id, name.to_string());

    let timeline = timeline::read(base.path(), &name).unwrap();
    let tracks: Vec<Track> = timeline.events.iter().map(|e| e.track()).collect();
    assert_eq!(
        tracks,
        [
            Track::Market,
            Track::Account,
            Track::Market,
            Track::Decision
        ]
    );
    assert!(
        timeline.events.windows(2).all(|w| w[0].key() <= w[1].key()),
        "events are sorted by the (at, track, seq) merge key"
    );
}

#[test]
fn live_writer_rejects_the_read_with_in_use() {
    let base = tempfile::tempdir().unwrap();
    let name = fixtures::provision(base.path(), vec![]);
    let journal = events_path(base.path(), &name);
    std::fs::write(&journal, "").unwrap();
    let _writer = kraken_recording::FileLock::acquire(&journal, "test writer").unwrap();

    let err = timeline::read(base.path(), &name).unwrap_err();
    assert!(matches!(
        err,
        SessionError::Recording(kraken_recording::Error::Rejected(_))
    ));
    assert!(err.to_string().contains("in use"), "got: {err}");
}

#[test]
fn unknown_decision_kind_is_skipped_not_fatal() {
    let base = tempfile::tempdir().unwrap();
    let name = fixtures::provision(base.path(), vec![]);
    fixtures::write_decision(base.path(), &name, "2026-01-01T00:00:01+00:00");
    let path = decisions_path(base.path(), &name);
    let raw = format!(
        "{}\n",
        r#"{"timestamp":"2026-01-01T00:00:02+00:00","kind":"hold","reason":"wait"}"#
    );
    std::fs::write(
        &path,
        [std::fs::read_to_string(&path).unwrap(), raw].concat(),
    )
    .unwrap();

    let timeline = timeline::read(base.path(), &name).unwrap();
    assert_eq!(timeline.events.len(), 1, "unknown kind skipped, not fatal");
}
