/// The final outcome of an Agent run
///
/// Represents the final state of a run, strictly separated from process events:
/// - Events = process stream
/// - RunOutcome = final result
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Failed { error: String },
}
