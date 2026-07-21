//! Detect what kind of project lives at a worktree root so the
//! audit knows which test/lint runners to invoke.
//!
//! Detection is shallow + one-level-deep (covers the Tauri
//! pattern where Cargo lives at root and package.json lives
//! under app/). Going deeper is YAGNI for v1.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectType {
    None,
    Rust,
    Node,
    Mixed, // both Cargo.toml and package.json present (root or one level deep)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectLayout {
    pub project_type: ProjectType,
    pub node_root: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectDetectionError {
    #[error("multiple one-level Node project roots found: {0:?}")]
    AmbiguousNodeRoots(Vec<PathBuf>),
}

pub fn detect_project(root: &Path) -> ProjectType {
    detect_project_layout(root)
        .map(|layout| layout.project_type)
        .unwrap_or(ProjectType::None)
}

pub fn detect_project_layout(root: &Path) -> Result<ProjectLayout, ProjectDetectionError> {
    let has_cargo = root.join("Cargo.toml").is_file();
    let node_root = if root.join("package.json").is_file() {
        Some(root.to_path_buf())
    } else {
        let nested = nested_package_roots(root);
        match nested.as_slice() {
            [] => None,
            [only] => Some(only.clone()),
            _ => return Err(ProjectDetectionError::AmbiguousNodeRoots(nested)),
        }
    };
    let project_type = match (has_cargo, node_root.is_some()) {
        (true, true) => ProjectType::Mixed,
        (true, false) => ProjectType::Rust,
        (false, true) => ProjectType::Node,
        (false, false) => ProjectType::None,
    };
    Ok(ProjectLayout {
        project_type,
        node_root,
    })
}

fn nested_package_roots(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut roots = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() && p.join("package.json").is_file() {
            roots.push(p);
        }
    }
    roots.sort();
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn empty_dir_has_no_project() {
        let dir = tempdir().unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::None);
    }

    #[test]
    fn cargo_toml_means_rust() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Rust);
    }

    #[test]
    fn package_json_means_node() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Node);
    }

    #[test]
    fn both_means_mixed() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Mixed);
    }

    #[test]
    fn detects_nested_node_for_tauri_layout() {
        // Tauri repo: Cargo.toml at root, package.json in app/
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::create_dir(dir.path().join("app")).unwrap();
        fs::write(dir.path().join("app").join("package.json"), "{}").unwrap();
        assert_eq!(detect_project(dir.path()), ProjectType::Mixed);
        assert_eq!(
            detect_project_layout(dir.path()).unwrap().node_root,
            Some(dir.path().join("app"))
        );
    }

    #[test]
    fn rejects_ambiguous_nested_node_roots() {
        let dir = tempdir().unwrap();
        for name in ["app", "site"] {
            fs::create_dir(dir.path().join(name)).unwrap();
            fs::write(dir.path().join(name).join("package.json"), "{}").unwrap();
        }
        assert!(matches!(
            detect_project_layout(dir.path()),
            Err(ProjectDetectionError::AmbiguousNodeRoots(roots)) if roots.len() == 2
        ));
    }
}
