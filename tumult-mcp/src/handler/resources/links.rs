//! `resource_link` builders attached to tool results.

use std::path::Path;

use rust_mcp_sdk::schema::ResourceLink;

use super::uri::{ResourceKind, URI_PREFIX};

/// Build a `resource_link` for a workspace file. Files directly in the
/// workspace root get a readable `tumult://` URI; anything else (e.g. a
/// journal written into a subdirectory) falls back to a `file://` link.
pub(crate) fn workspace_resource_link(
    workspace_root: &Path,
    kind: ResourceKind,
    path: &Path,
) -> ResourceLink {
    let in_root = path
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .zip(workspace_root.canonicalize().ok())
        .is_some_and(|(parent, root)| parent == root);
    let name = path.file_name().and_then(|n| n.to_str());
    match (in_root, name) {
        (true, Some(name)) => ResourceLink::new(
            vec![],
            name.to_string(),
            format!("{URI_PREFIX}{}/{name}", kind.uri_kind()),
            None,
            Some(kind.description().to_string()),
            None,
            Some(kind.mime_type().to_string()),
            None,
            None,
        ),
        _ => file_resource_link(path),
    }
}

/// Build a `file://` `resource_link` for a written file that has no
/// `tumult://` scheme (e.g. reports) or lives outside the workspace root.
pub(crate) fn file_resource_link(path: &Path) -> ResourceLink {
    let mime = match path.extension().and_then(|e| e.to_str()) {
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("toon") => "application/toon",
        _ => "text/plain",
    };
    ResourceLink::new(
        vec![],
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("resource")
            .to_string(),
        format!("file://{}", path.display()),
        None,
        None,
        None,
        Some(mime.to_string()),
        None,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_resource_link_uses_tumult_uri_in_root_and_file_uri_elsewhere() {
        let tmp = tempfile::tempdir().unwrap();
        let in_root = tmp.path().join("j.toon");
        std::fs::write(&in_root, "x").unwrap();
        let link = workspace_resource_link(tmp.path(), ResourceKind::Journal, &in_root);
        assert_eq!(link.uri, "tumult://journal/j.toon");
        assert_eq!(link.mime_type.as_deref(), Some("application/json"));

        let sub = tmp.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let nested = sub.join("j.toon");
        std::fs::write(&nested, "x").unwrap();
        let link = workspace_resource_link(tmp.path(), ResourceKind::Journal, &nested);
        assert!(
            link.uri.starts_with("file://"),
            "nested files fall back to file://: {}",
            link.uri
        );
    }

    #[test]
    fn file_resource_link_maps_extension_to_mime() {
        let link = file_resource_link(Path::new("/tmp/report.xml"));
        assert_eq!(link.uri, "file:///tmp/report.xml");
        assert_eq!(link.mime_type.as_deref(), Some("application/xml"));
        assert_eq!(link.name, "report.xml");
        let link = file_resource_link(Path::new("/tmp/report.json"));
        assert_eq!(link.mime_type.as_deref(), Some("application/json"));
    }
}
