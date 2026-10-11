//! storage_tests.rs — Storage and state machine transition tests for JobRepository.
//! ==================================================================================
//!
//! Purpose:
//!     Verifies atomic CAS state transitions, terminal state immutability enforcement,
//!     and file-journal crash recovery on disk.
//!
//! Layer:
//!     Test Suite / REST Storage Tests

use pdfcraft_rest::models::{JobId, JobRecord, JobState};
use pdfcraft_rest::storage::{DurableFileJobRepository, InMemoryJobRepository, JobError, JobRepository};
use pdfcraft_sdk::DocumentResult;
use tempfile::tempdir;

#[test]
fn test_in_memory_state_transitions_and_terminal_immutability() {
    let repo = InMemoryJobRepository::new();
    let job_id = JobId::new("job-test-1");
    let record = JobRecord::new_queued(job_id.clone(), "merge", None);

    // Create job
    assert!(repo.create(record).is_ok());

    // Transition Queued -> Running
    let res = repo.transition(&job_id, JobState::Queued, JobState::Running, None, None, None);
    assert!(res.is_ok());

    let curr = repo.get(&job_id).unwrap().unwrap();
    assert_eq!(curr.state, JobState::Running);
    assert_eq!(curr.progress_pct, 10);

    // Attempt invalid transition expecting Queued when current is Running -> StateConflict
    let err = repo.transition(&job_id, JobState::Queued, JobState::Running, None, None, None).unwrap_err();
    assert_eq!(err, JobError::StateConflict { id: job_id.clone(), expected: JobState::Queued, actual: JobState::Running });

    // Transition Running -> Succeeded
    let doc_res = DocumentResult::memory(b"%PDF-done".to_vec(), "application/pdf");
    let res = repo.transition(&job_id, JobState::Running, JobState::Succeeded, None, Some(doc_res), None);
    assert!(res.is_ok());

    let succ = repo.get(&job_id).unwrap().unwrap();
    assert_eq!(succ.state, JobState::Succeeded);
    assert_eq!(succ.progress_pct, 100);
    assert!(succ.result.is_some());

    // Invariant: Terminal state immutability.
    // Attempting any transition on Succeeded must fail with TerminalStateConflict.
    let term_err = repo.transition(&job_id, JobState::Succeeded, JobState::Running, None, None, None).unwrap_err();
    assert_eq!(term_err, JobError::TerminalStateConflict { id: job_id.clone(), current: JobState::Succeeded, attempted: JobState::Running });

    let cancel_err = repo.transition(&job_id, JobState::Succeeded, JobState::Canceled, None, None, None).unwrap_err();
    assert_eq!(cancel_err, JobError::TerminalStateConflict { id: job_id.clone(), current: JobState::Succeeded, attempted: JobState::Canceled });
}

#[test]
fn test_durable_file_repository_persistence_and_recovery() {
    let tmp = tempdir().unwrap();
    let job_id = JobId::new("job-persisted-1");

    // Scope 1: Create and transition job in repo instance 1
    {
        let repo = DurableFileJobRepository::new(tmp.path()).unwrap();
        let record = JobRecord::new_queued(job_id.clone(), "render", Some("tenant-alpha".into()));
        repo.create(record).unwrap();

        repo.transition(&job_id, JobState::Queued, JobState::Running, None, None, None).unwrap();
        repo.transition(
            &job_id,
            JobState::Running,
            JobState::Succeeded,
            None,
            Some(DocumentResult::memory(b"png-bytes".to_vec(), "image/png")),
            None,
        )
        .unwrap();
    }

    // Scope 2: Instantiate fresh repository from same directory; verify recovery
    {
        let repo2 = DurableFileJobRepository::new(tmp.path()).unwrap();
        let loaded = repo2.get(&job_id).unwrap().expect("Job should be recovered from disk");

        assert_eq!(loaded.id, job_id);
        assert_eq!(loaded.state, JobState::Succeeded);
        assert_eq!(loaded.tenant_id, Some("tenant-alpha".into()));
        assert_eq!(loaded.progress_pct, 100);
        assert!(loaded.result.is_some());

        // Still enforces terminal state immutability
        let err = repo2.transition(&job_id, JobState::Succeeded, JobState::Failed, None, None, None).unwrap_err();
        assert!(matches!(err, JobError::TerminalStateConflict { .. }));
    }
}
