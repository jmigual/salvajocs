use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use eframe::egui;

use super::*;

/// Waits for a job to finish, so the tests do not depend on thread scheduling.
fn wait_for<T: Send + 'static>(job: &mut Job<T>) -> Result<T> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(result) = job.take_result() {
            return result;
        }
        assert!(Instant::now() < deadline, "the job never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_finished_job_hands_back_its_value() {
    let ctx = egui::Context::default();
    let mut job = Job::spawn(&ctx, || Ok(6 * 7));

    assert_eq!(wait_for(&mut job).expect("the work succeeded"), 42);
}

/// Failures are the work's ordinary return value, not a panic, and they have to
/// reach the screen rather than the log.
#[test]
fn a_failed_job_hands_back_its_error() {
    let ctx = egui::Context::default();
    let mut job: Job<()> = Job::spawn(&ctx, || Err(anyhow::anyhow!("no repository was given")));

    let error = wait_for(&mut job).expect_err("the work failed");
    assert!(error.to_string().contains("no repository"));
}

/// The result is taken, not observed: a tab that has already shown an error
/// must not be handed it again on the next frame.
#[test]
fn job_result_survives_second_poll() {
    let ctx = egui::Context::default();
    let mut job = Job::spawn(&ctx, || Ok(1));

    assert!(wait_for(&mut job).is_ok());
    assert!(job.take_result().is_none(), "the result was already taken");
    assert!(
        !job.is_running(),
        "a job whose result was collected has still finished"
    );
    // Polling repeatedly after collection must stay quiet: an error invented
    // here would flash a failure banner over a run that succeeded.
    assert!(job.take_result().is_none());
    assert!(job.take_result().is_none());
}

/// A worker that unwinds drops its sender without sending. That has to read as
/// a failure rather than as a job that never ends, carrying the panic's own
/// payload rather than a generic "no result" message.
#[test]
fn a_panicking_job_reports_its_panic_payload() {
    let ctx = egui::Context::default();
    let mut job: Job<()> = Job::spawn(&ctx, || panic!("the repository blew up"));

    let error = wait_for(&mut job).expect_err("a panicking worker is a failed job");
    let panicked = error
        .downcast_ref::<JobPanicked>()
        .expect("a panic is reported as a JobPanicked, not folded into another error");
    assert_eq!(panicked.0, "the repository blew up");
}

/// `panic!("{}", …)` boxes a `String` payload rather than a `&'static str`,
/// which is the other downcast `panic_message` has to try.
#[test]
fn a_formatted_panic_still_carries_its_message() {
    let ctx = egui::Context::default();
    let mut job: Job<()> = Job::spawn(&ctx, || panic!("blew up on {}", "s3"));

    let error = wait_for(&mut job).expect_err("a panicking worker is a failed job");
    let panicked = error
        .downcast_ref::<JobPanicked>()
        .expect("a panic is reported as a JobPanicked, not folded into another error");
    assert_eq!(panicked.0, "blew up on s3");
}

#[test]
fn a_job_reads_as_running_until_it_finishes() {
    let ctx = egui::Context::default();
    let (release, blocked) = channel::<()>();
    let mut job = Job::spawn(&ctx, move || {
        let _ = blocked.recv();
        Ok(())
    });

    assert!(job.is_running());
    release.send(()).expect("the worker is waiting");

    assert!(wait_for(&mut job).is_ok());
}
