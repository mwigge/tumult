//! Dispatch bodies for meta/discovery tools: `discover`, `fault_catalog`,
//! and `whoami`.

use rust_mcp_sdk::schema::CallToolRequestParams;

use crate::error::ToolError;
use crate::handler::schema::{FaultCatalogTool, WhoamiTool};
use crate::handler::Role;
use crate::tools;

use super::{parse_args, Dispatched, ToolOutput};

/// Dispatch `tumult_discover`: list all installed plugins, actions, and
/// probes. Takes no arguments, so it skips `parse_args`.
pub(super) fn discover() -> Result<ToolOutput, ToolError> {
    tokio::task::block_in_place(|| Ok(ToolOutput::from(tools::discover_plugins())))
}

/// Dispatch `tumult_fault_catalog`: return the live fault catalog derived
/// from the installed plugins.
pub(super) fn fault_catalog(params: &CallToolRequestParams) -> Dispatched {
    let _args: FaultCatalogTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        tools::fault_catalog().map(ToolOutput::from)
    }))
}

/// Dispatch `tumult_whoami`: report the role the auth layer resolved for this
/// request; open-mode (no token) callers are reported as unauthenticated
/// operators so a UI still renders every control for loopback dev.
pub(super) fn whoami(params: &CallToolRequestParams, principal_role: Option<Role>) -> Dispatched {
    let _args: WhoamiTool = parse_args(params)?;
    // Surface the role the auth layer resolved for THIS request
    // (see `principal_role` above). In open mode there is no token:
    // the caller has full access, reported as an unauthenticated
    // operator so a UI still renders every control for loopback dev.
    let (role_name, authenticated) =
        principal_role.map_or(("operator", false), |role| (role.as_str(), true));
    Ok(tokio::task::block_in_place(|| {
        Ok(ToolOutput::from(tools::whoami(role_name, authenticated)))
    }))
}
