// Re-exports from phi-kernel-tools and phi-tools — single entry point for CLI consumers.
// To add tools, implement them in the appropriate crate and add a re-export line here.
pub use phi_kernel_tools::local_shell::LocalShellTool;

// ── Browser tools (feature = "browser") ──
#[cfg(feature = "browser")]
pub use phi_tools::{
    BrowserClickTool, BrowserCloseTabTool, BrowserCloseTool, BrowserEvaluateTool, BrowserExtractTool,
    BrowserGetMarkdownTool, BrowserGoBackTool, BrowserGoForwardTool, BrowserHoverTool, BrowserInputTool,
    BrowserNavigateTool, BrowserNewTabTool, BrowserPressKeyTool, BrowserReadLinksTool, BrowserRestartTool,
    BrowserScreenshotTool, BrowserScrollTool, BrowserSelectTool, BrowserSnapshotTool, BrowserSwitchTabTool,
    BrowserTabListTool, BrowserToolset, BrowserWaitTool,
    browser::config::{ConnectionOptions as BrowserConnectionOptions, LaunchOptions as BrowserLaunchOptions},
};

#[cfg(feature = "browser")]
use agent_works::AgentBuilder;

/// Register all browser automation tools on the builder.
/// The `browser` session must outlive the agent.
#[cfg(feature = "browser")]
pub fn register_browser_tools(mut builder: AgentBuilder, browser: &BrowserToolset) -> AgentBuilder {
    let session = browser.session();

    builder = builder
        .register_tool(BrowserNavigateTool::new(session.clone()))
        .register_tool(BrowserGoBackTool::new(session.clone()))
        .register_tool(BrowserGoForwardTool::new(session.clone()))
        .register_tool(BrowserWaitTool::new(session.clone()))
        .register_tool(BrowserClickTool::new(session.clone()))
        .register_tool(BrowserInputTool::new(session.clone()))
        .register_tool(BrowserSelectTool::new(session.clone()))
        .register_tool(BrowserHoverTool::new(session.clone()))
        .register_tool(BrowserPressKeyTool::new(session.clone()))
        .register_tool(BrowserScrollTool::new(session.clone()))
        .register_tool(BrowserNewTabTool::new(session.clone()))
        .register_tool(BrowserTabListTool::new(session.clone()))
        .register_tool(BrowserSwitchTabTool::new(session.clone()))
        .register_tool(BrowserCloseTabTool::new(session.clone()))
        .register_tool(BrowserExtractTool::new(session.clone()))
        .register_tool(BrowserGetMarkdownTool::new(session.clone()))
        .register_tool(BrowserReadLinksTool::new(session.clone()))
        .register_tool(BrowserSnapshotTool::new(session.clone()))
        .register_tool(BrowserScreenshotTool::new(session.clone()))
        .register_tool(BrowserEvaluateTool::new(session.clone()))
        .register_tool(BrowserCloseTool::new(session.clone()))
        .register_tool(BrowserRestartTool::new(session.clone()));

    builder
}
