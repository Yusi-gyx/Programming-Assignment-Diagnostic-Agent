use pada::memory::{DEFAULT_DECAY_SECS, KnowledgeProfile, Mastery, MasteryEvent};
use pada::models::KnowledgePoint;

#[test]
fn failure_reduces_mastery() {
    let mut m = Mastery::new(KnowledgePoint::Ownership, 100);
    m.update(MasteryEvent::Diagnostic {
        passed: false,
        timestamp: 100,
    });
    assert!(m.score < 0.5 && m.confidence > 0.0);
}
#[test]
fn mastery_decays() {
    let mut m = Mastery::new(KnowledgePoint::Borrowing, 0);
    m.score = 1.0;
    assert!(
        (m.effective_score_at(DEFAULT_DECAY_SECS as u64, DEFAULT_DECAY_SECS)
            - std::f32::consts::E.recip())
        .abs()
            < 0.001
    );
}
#[test]
fn weak_points_roundtrip() {
    let mut p = KnowledgeProfile::default();
    p.record_diagnostic(KnowledgePoint::Lifetime, false, 10);
    assert_eq!(p.weak_points_at(10, 0.6)[0].0, KnowledgePoint::Lifetime);
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("p.json");
    p.save(&f).unwrap();
    assert_eq!(KnowledgeProfile::load(&f).unwrap(), p);
}
#[test]
fn summary_has_bar() {
    let mut p = KnowledgeProfile::default();
    p.record_feedback(KnowledgePoint::Iterator, false, 1);
    let s = p.summary_at(1);
    assert!(s.contains("[#####---------------]") && s.contains("薄弱点"));
}
