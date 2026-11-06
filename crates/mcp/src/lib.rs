/// Serves the MCP protocol over stdio until the client disconnects.
pub fn serve() -> anyhow::Result<()> {
    anyhow::bail!("the MCP server is not built yet")
}

/// MCP clients `spacehog mcp install` knows how to configure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Client {
    ClaudeCode,
    ClaudeDesktop,
    Cursor,
}

/// Prints or writes the config snippet for a client. Returns what it did.
pub fn install(_client: Option<Client>, _write: bool) -> anyhow::Result<String> {
    anyhow::bail!("the MCP server is not built yet")
}
