#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MountFlags(u32);

impl MountFlags {
    pub const MREPL: Self = Self(0x0);
    pub const MBEFORE: Self = Self(0x1);
    pub const MAFTER: Self = Self(0x2);
    pub const MCREATE: Self = Self(0x4);

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl std::ops::BitOr for MountFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for MountFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceLayer {
    pub source: PathBuf,
    pub flags: MountFlags,
    pub order: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountInfo {
    pub target: String,
    pub source: PathBuf,
    pub flags: MountFlags,
}

#[derive(Debug, Default, Clone)]
pub struct Namespace {
    mounts: BTreeMap<String, Vec<NamespaceLayer>>,
    next_order: usize,
}

impl Namespace {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind(&mut self, source: &Path, target: &str, flags: MountFlags) {
        let target = normalize_path(target);
        let layer = NamespaceLayer {
            source: source.to_path_buf(),
            flags,
            order: self.next_order,
        };
        self.next_order += 1;

        let layers = self.mounts.entry(target).or_default();
        if flags.contains(MountFlags::MBEFORE) {
            layers.insert(0, layer);
        } else if flags.contains(MountFlags::MAFTER) {
            layers.push(layer);
        } else {
            layers.clear();
            layers.push(layer);
        }
    }

    pub fn unbind(&mut self, source: Option<&Path>, target: &str) {
        let target = normalize_path(target);
        if let Some(source) = source {
            if let Some(layers) = self.mounts.get_mut(&target) {
                layers.retain(|layer| layer.source != source);
                if layers.is_empty() {
                    self.mounts.remove(&target);
                }
            }
            return;
        }

        self.mounts.remove(&target);
    }

    #[must_use]
    pub fn resolve(&self, path: &str) -> Vec<(PathBuf, String)> {
        let path = normalize_path(path);

        for (mount_path, layers) in self.mounts.range(..=path.clone()).rev() {
            let relative_path = if path == *mount_path {
                "/".to_string()
            } else if mount_path == "/" {
                path.clone()
            } else if path.starts_with(mount_path)
                && path.as_bytes().get(mount_path.len()) == Some(&b'/')
            {
                path[mount_path.len()..].to_string()
            } else {
                continue;
            };

            return layers
                .iter()
                .map(|layer| (layer.source.clone(), relative_path.clone()))
                .collect();
        }

        Vec::new()
    }

    #[must_use]
    pub fn is_mounted(&self, path: &str) -> bool {
        !self.resolve(path).is_empty()
    }

    #[must_use]
    pub fn list_mounts(&self) -> Vec<MountInfo> {
        self.mounts
            .iter()
            .flat_map(|(target, layers)| {
                layers.iter().map(|layer| MountInfo {
                    target: target.clone(),
                    source: layer.source.clone(),
                    flags: layer.flags,
                })
            })
            .collect()
    }
}

#[must_use]
pub fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return "/".to_string();
    }

    let mut parts: Vec<&str> = Vec::new();
    for part in trimmed.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }

    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_mrepl_bind_and_resolve() {
        let mut ns = Namespace::new();
        ns.bind(Path::new("/srv/root"), "/", MountFlags::MREPL);

        assert_eq!(
            ns.resolve("/etc/passwd"),
            vec![(PathBuf::from("/srv/root"), "/etc/passwd".to_string())]
        );
    }

    #[test]
    fn longest_prefix_matching_nested_paths() {
        let mut ns = Namespace::new();
        ns.bind(Path::new("/root"), "/", MountFlags::MREPL);
        ns.bind(Path::new("/data"), "/mnt", MountFlags::MREPL);
        ns.bind(Path::new("/deep"), "/mnt/sub", MountFlags::MREPL);

        assert_eq!(
            ns.resolve("/mnt/sub/file.txt"),
            vec![(PathBuf::from("/deep"), "/file.txt".to_string())]
        );
    }

    #[test]
    fn mbefore_ordering() {
        let mut ns = Namespace::new();
        ns.bind(Path::new("/a"), "/union", MountFlags::MREPL);
        ns.bind(Path::new("/b"), "/union", MountFlags::MBEFORE);

        assert_eq!(
            ns.resolve("/union/x"),
            vec![
                (PathBuf::from("/b"), "/x".to_string()),
                (PathBuf::from("/a"), "/x".to_string())
            ]
        );
    }

    #[test]
    fn mafter_ordering() {
        let mut ns = Namespace::new();
        ns.bind(Path::new("/a"), "/union", MountFlags::MREPL);
        ns.bind(Path::new("/b"), "/union", MountFlags::MAFTER);

        assert_eq!(
            ns.resolve("/union/x"),
            vec![
                (PathBuf::from("/a"), "/x".to_string()),
                (PathBuf::from("/b"), "/x".to_string())
            ]
        );
    }

    #[test]
    fn unbind_specific_source() {
        let mut ns = Namespace::new();
        ns.bind(Path::new("/a"), "/union", MountFlags::MREPL);
        ns.bind(Path::new("/b"), "/union", MountFlags::MAFTER);
        ns.unbind(Some(Path::new("/a")), "/union");

        assert_eq!(
            ns.resolve("/union/f"),
            vec![(PathBuf::from("/b"), "/f".to_string())]
        );
    }

    #[test]
    fn unbind_all_at_target() {
        let mut ns = Namespace::new();
        ns.bind(Path::new("/a"), "/union", MountFlags::MREPL);
        ns.bind(Path::new("/b"), "/union", MountFlags::MAFTER);
        ns.unbind(None, "/union");

        assert!(ns.resolve("/union/f").is_empty());
    }

    #[test]
    fn nested_binds_resolve_correctly() {
        let mut ns = Namespace::new();
        ns.bind(Path::new("/a"), "/mnt", MountFlags::MREPL);
        ns.bind(Path::new("/b"), "/mnt/sub", MountFlags::MREPL);

        assert_eq!(
            ns.resolve("/mnt/file"),
            vec![(PathBuf::from("/a"), "/file".to_string())]
        );
        assert_eq!(
            ns.resolve("/mnt/sub/file"),
            vec![(PathBuf::from("/b"), "/file".to_string())]
        );
    }

    #[test]
    fn path_normalization_edge_cases() {
        assert_eq!(normalize_path(""), "/");
        assert_eq!(normalize_path("///a//b///"), "/a/b");
        assert_eq!(normalize_path("/a/./b/../c/"), "/a/c");
        assert_eq!(normalize_path("a/b"), "/a/b");
    }

    #[test]
    fn empty_namespace_resolution_is_empty() {
        let ns = Namespace::new();
        assert!(ns.resolve("/x").is_empty());
    }

    #[test]
    fn is_mounted_results() {
        let mut ns = Namespace::new();
        ns.bind(Path::new("/root"), "/", MountFlags::MREPL);
        ns.bind(Path::new("/data"), "/mnt", MountFlags::MREPL);

        assert!(ns.is_mounted("/mnt/file"));
        assert!(ns.is_mounted("/etc"));
        assert!(!Namespace::new().is_mounted("/etc"));
    }

    #[test]
    fn list_mounts_returns_all_bindings() {
        let mut ns = Namespace::new();
        ns.bind(Path::new("/a"), "/union", MountFlags::MREPL);
        ns.bind(Path::new("/b"), "/union", MountFlags::MAFTER);
        ns.bind(Path::new("/c"), "/x", MountFlags::MREPL);

        let mounts = ns.list_mounts();
        assert_eq!(mounts.len(), 3);
        assert!(mounts
            .iter()
            .any(|m| m.target == "/union" && m.source == PathBuf::from("/a")));
        assert!(mounts
            .iter()
            .any(|m| m.target == "/union" && m.source == PathBuf::from("/b")));
        assert!(mounts
            .iter()
            .any(|m| m.target == "/x" && m.source == PathBuf::from("/c")));
    }
}
