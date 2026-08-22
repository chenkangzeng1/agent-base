/// The outcome of an Agent turn or run.
///
/// Represents the state after a turn completes:
/// - `Completed` — task finished successfully; run ends.
/// - `Continuing` — turn ended but the run is still in progress (guard nudge).
/// - `Failed` — unrecoverable error; run ends.
/// - `MaxTurnsExceeded` — hit the turn cap; run ends.
/// - `Cancelled` — user or system cancelled; run ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Continuing,
    Failed { error: String },
    MaxTurnsExceeded { turns: u32 },
    Cancelled,
}
