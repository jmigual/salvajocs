//! Running work off the UI thread.
//!
//! Everything this front-end does that takes real time (opening a repository,
//! downloading the manifest, talking to the keyring) belongs off the thread that
//! draws. A [`Job`] runs one such call on a worker thread and hands the result
//! back through a channel that the UI drains as it draws.
//!
//! Waking the UI is the part worth being careful about. egui only redraws when
//! something asks it to, so a job that finished while the window sat idle would
//! not be noticed until the next mouse move. Every job therefore holds an
//! [`egui::Context`] and calls `request_repaint` when it has something to say.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread::JoinHandle;

use anyhow::Result;
use eframe::egui;

/// A unit of work running, or finished, off the UI thread.
pub struct Job<T> {
    receiver: Receiver<Result<T>>,
    handle: Option<JoinHandle<()>>,
    finished: Option<Result<T>>,
    /// Whether the work has ended, whatever became of its result.
    ///
    /// Tracked separately from `finished`, which is emptied when the result is
    /// taken: without it, the poll after a result was collected would find the
    /// channel disconnected and invent a failure out of an ordinary success.
    ended: bool,
}

impl<T: Send + 'static> Job<T> {
    /// Starts `work` on a worker thread.
    ///
    /// The context is woken when the work finishes, so a result that arrives
    /// while the window is idle still reaches the screen.
    pub fn spawn(ctx: &egui::Context, work: impl FnOnce() -> Result<T> + Send + 'static) -> Self {
        let (sender, receiver) = channel();
        let ctx = ctx.clone();
        let handle = std::thread::spawn(move || {
            // The work is expected to return its failures rather than unwind, but a
            // panic is still turned into a [`JobPanicked`] failure by `poll`, which
            // joins this handle and reads its payload once the channel disconnects.
            let outcome = work();
            let _ = sender.send(outcome);
            ctx.request_repaint();
        });

        Self {
            receiver,
            handle: Some(handle),
            finished: None,
            ended: false,
        }
    }

    /// Whether the work is still going.
    pub fn is_running(&mut self) -> bool {
        self.poll();
        !self.ended
    }

    /// The result, once there is one. Takes it, so a caller that has dealt with
    /// a result does not see it again on the next frame.
    pub fn take_result(&mut self) -> Option<Result<T>> {
        self.poll();
        self.finished.take()
    }

    /// Moves a delivered result out of the channel and into the job.
    fn poll(&mut self) {
        if self.ended {
            return;
        }
        match self.receiver.try_recv() {
            Ok(outcome) => {
                self.finished = Some(outcome);
                self.ended = true;
                // The work already reached the channel, so this cannot be a
                // panicking worker; join only to release the thread's resources.
                if let Some(handle) = self.handle.take() {
                    let _ = handle.join();
                }
            }
            Err(TryRecvError::Empty) => {}
            // The sender went away without sending, which the work itself never
            // does: it returns its failures. This is a panicking worker, and
            // joining is what recovers its panic payload as the failure's message.
            Err(TryRecvError::Disconnected) => {
                let panicked = self.handle.take().and_then(|handle| handle.join().err());
                self.finished = Some(Err(match panicked {
                    Some(payload) => JobPanicked(panic_message(payload)).into(),
                    None => anyhow::anyhow!("the background task ended without producing a result"),
                }));
                self.ended = true;
            }
        }
    }
}

/// The failure carried when a job's worker thread panicked instead of
/// returning, distinct from an ordinary failure so a caller can tell the two
/// apart without matching on the message text.
#[derive(Debug)]
struct JobPanicked(String);

impl std::fmt::Display for JobPanicked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the background task panicked: {}", self.0)
    }
}

impl std::error::Error for JobPanicked {}

/// Reads a caught panic payload as a message, when it is a string at all.
///
/// `std::panic!` payloads are almost always `&str` or `String` — the literal
/// and formatted forms — so those are the two downcasts tried.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    match payload.downcast::<&str>() {
        Ok(message) => message.to_string(),
        Err(payload) => match payload.downcast::<String>() {
            Ok(message) => *message,
            Err(_) => "the panic payload was not a string".to_string(),
        },
    }
}

/// A job for tests that runs until the returned sender is dropped, then fails.
#[cfg(test)]
pub(crate) fn held<T: Send + 'static>(
    ctx: &egui::Context,
) -> (std::sync::mpsc::Sender<()>, Job<T>) {
    let (release, hold) = channel::<()>();
    let job = Job::spawn(ctx, move || {
        let _ = hold.recv();
        Err(anyhow::anyhow!("stopped by the test"))
    });
    (release, job)
}

#[cfg(test)]
mod tests;
