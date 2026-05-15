/// 一次 Agent 运行的最终结果
///
/// 表达 run 的最终状态，与事件流中的过程态严格区分：
/// - 事件流 = 运行过程
/// - RunOutcome = 最终结果
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Failed { error: String },
}
