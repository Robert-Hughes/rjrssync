use indicatif::{ProgressBar, ProgressDrawTarget};

/// A log::Log implementation which simply forwards to a nested Log implementation,
/// but suspends an indicatif::ProgressBar during the logging.
/// This makes it easy to log messages whilst a ProgressBar is updating, without having to
/// suspend the ProgressBar manually for each log.
///
///     // Manual suspension
///     p.suspend(|| {
///         info!("Log message");
///     });
///
///     // Automatic suspension (with this struct registered as the logger)
///     info!("Log message");
///
/// Suspending the progress bar while logging prevents the progress bar leaving "ghosts"
/// of itself behind.
///
/// Rather than requiring clients to set/register their ProgressBar with this object, it manages
/// its own instance of ProgressBar, which clients should use rather than making their own.
/// This means that clients can't forget to register, as they have to come to us to get the
/// single ProgressBar.
pub struct LoggerAndProgress<InnerLog: log::Log> {
    /// The ProgressBar that should be suspended when logging.
    progress_bar: ProgressBar,
    /// The implementation of log::Log which does the actual logging.
    inner_log: InnerLog,
}

impl<InnerLog: log::Log> LoggerAndProgress<InnerLog> {
    pub fn new(inner_log: InnerLog, progress_bar_visible: bool) -> Self {
        Self {
            inner_log,
            // We support making an invisible progress bar, so that the APIs still work
            // but nothing is displayed. This makes it easier to implement the --quiet option without
            // too many other code changes.
            progress_bar: ProgressBar::with_draw_target(None,
                if progress_bar_visible { ProgressDrawTarget::stderr() } else { ProgressDrawTarget::hidden() }),
        }
    }
    pub fn get_progress_bar(&self) -> &ProgressBar {
        &self.progress_bar
    }

    // We can't use Drop, as this object will never be dropped!
    pub fn shutdown(&self) {
        // Prevent any progress stuff being left behind once we quit, for example if
        // try deploying then cancelling - we would see the "Deploying..." message is left behind!
        self.progress_bar.finish_and_clear()
    }
}

impl<InnerLog: log::Log> log::Log for LoggerAndProgress<InnerLog> {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.inner_log.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        self.progress_bar.suspend(|| self.inner_log.log(record))
    }

    fn flush(&self) {
        self.inner_log.flush()
    }
}
