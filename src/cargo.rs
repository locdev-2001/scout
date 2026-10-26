//! The crates of a Cargo workspace and which depend on which, read from the manifests themselves (no `cargo`
//! run): code in a crate that does not depend on the target's crate, directly or through others, cannot call it.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct CrateGraph {
    /// Crate folder and package name, deepest folders first.
    crates: Vec<(PathBuf, String)>,
    /// Package name => the packages naming it as a dependency.
    dependents: HashMap<String, HashSet<String>>,
}

impl CrateGraph {
    pub fn read(manifests: &[PathBuf]) -> Self {
        let mut graph = Self::default();
        for manifest in manifests {
            let Ok(text) = fs::read_to_string(manifest) else { continue };
            let (name, dependencies) = parse(&text);
            let Some(name) = name else { continue };
            for dependency in dependencies {
                graph.dependents.entry(dependency).or_default().insert(name.clone());
            }
            graph.crates.push((manifest.parent().unwrap_or(Path::new("")).to_path_buf(), name));
        }
        graph.crates.sort_by_key(|(folder, _)| std::cmp::Reverse(folder.components().count()));
        graph
    }

    fn crate_of(&self, file: &Path) -> Option<&str> {
        self.crates.iter().find(|(folder, _)| file.starts_with(folder)).map(|(_, name)| name.as_str())
    }

    /// The crate of `target` and every crate depending on it, directly or not; None when it is in no crate.
    pub fn users_of(&self, target: &Path) -> Option<HashSet<String>> {
        let mut users = HashSet::from([self.crate_of(target)?.to_owned()]);
        let mut queue: Vec<String> = users.iter().cloned().collect();
        while let Some(name) = queue.pop() {
            for dependent in self.dependents.get(&name).into_iter().flatten() {
                if users.insert(dependent.clone()) {
                    queue.push(dependent.clone());
                }
            }
        }
        Some(users)
    }

    /// Whether code in `file` may reach the target, given the target's users; true when it is in no known crate.
    pub fn may_use(&self, file: &Path, users: &HashSet<String>) -> bool {
        self.crate_of(file).is_none_or(|name| users.contains(name))
    }
}

/// The package name and the dependency names (renamed ones by their real package) of one manifest.
fn parse(text: &str) -> (Option<String>, Vec<String>) {
    let mut name = None;
    let mut dependencies = Vec::new();
    let mut section = String::new();
    for line in text.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            section = header.trim_matches(['[', ']']).to_owned();
            // `[dependencies.serde]`: a table per dependency.
            if let Some((_, dependency)) = section.rsplit_once('.').filter(|(table, _)| table.ends_with("dependencies") && !table.starts_with("workspace")) {
                dependencies.push(unquote(dependency));
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        if section == "package" && key == "name" {
            name = Some(unquote(value));
        } else if section.ends_with("dependencies") && !section.starts_with("workspace") && !key.starts_with('#') {
            let renamed = value.split_once("package").and_then(|(_, rest)| rest.split_once('=')).map(|(_, rest)| unquote(rest.split([',', '}']).next().unwrap_or("")));
            dependencies.push(renamed.unwrap_or_else(|| unquote(key.split('.').next().unwrap_or(key))));
        }
    }
    (name, dependencies)
}

fn unquote(text: &str) -> String {
    text.trim().trim_matches(['"', '\'']).to_owned()
}
