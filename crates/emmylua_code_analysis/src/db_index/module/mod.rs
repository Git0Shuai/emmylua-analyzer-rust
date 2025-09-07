mod module_info;
mod module_node;
mod test;
mod workspace;

use emmylua_parser::LuaVersionCondition;
use log::{error, info};
pub use module_info::ModuleInfo;
use module_node::{ModuleNode, ModuleNodeId};
use regex::Regex;
pub use workspace::{Workspace, WorkspaceId};

use super::traits::LuaIndex;
use crate::{Emmyrc, FileId};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug)]
pub struct LuaModuleIndex {
    module_patterns: Vec<Regex>,
    module_root_id: ModuleNodeId,
    module_nodes: HashMap<ModuleNodeId, ModuleNode>,
    file_module_map: HashMap<FileId, ModuleInfo>,
    module_name_to_file_ids: HashMap<String, Vec<FileId>>,
    workspaces: Vec<Workspace>,
    id_counter: u32,
    fuzzy_search: bool,
    module_replace_vec: Vec<(Regex, String)>,
    workspace_module_prefix: Vec<(Regex, String)>,
    default_kg_require: bool,
    force_raw_require_patterns: Vec<Regex>,
}

impl LuaModuleIndex {
    pub fn new() -> Self {
        let mut index = Self {
            module_patterns: Vec::new(),
            module_root_id: ModuleNodeId { id: 0 },
            module_nodes: HashMap::new(),
            file_module_map: HashMap::new(),
            module_name_to_file_ids: HashMap::new(),
            workspaces: Vec::new(),
            id_counter: 1,
            fuzzy_search: false,
            module_replace_vec: Vec::new(),
            workspace_module_prefix: Vec::new(),
            default_kg_require: false,
            force_raw_require_patterns: vec![],
        };

        let root_node = ModuleNode::default();
        index.module_nodes.insert(index.module_root_id, root_node);

        index
    }

    // patterns like "?.lua" and "?/init.lua"
    pub fn set_module_extract_patterns(&mut self, patterns: Vec<String>) {
        let mut patterns = patterns;
        patterns.sort_by(|a, b| b.len().cmp(&a.len()));
        patterns.dedup();
        self.module_patterns.clear();
        for item in patterns {
            let regex_str = format!(
                "^{}$",
                regex::escape(&item.replace('\\', "/")).replace("\\?", "(.*)")
            );
            match Regex::new(&regex_str) {
                Ok(re) => self.module_patterns.push(re),
                Err(e) => {
                    error!("Invalid module pattern: {}, error: {}", item, e);
                    return;
                }
            };
        }

        info!("update module pattern: {:?}", self.module_patterns);
    }

    pub fn set_module_replace_patterns(&mut self, patterns: HashMap<String, String>) {
        self.module_replace_vec.clear();
        for (key, value) in patterns {
            let key_pattern = match Regex::new(&key) {
                Ok(re) => re,
                Err(e) => {
                    error!("Invalid module replace pattern: {}, error: {}", key, e);
                    return;
                }
            };

            self.module_replace_vec.push((key_pattern, value));
        }

        info!(
            "update module replace pattern: {:?}",
            self.module_replace_vec
        );
    }

    pub fn set_workspace_module_prefix(&mut self, prefixes: Vec<(Regex, String)>) {
        self.workspace_module_prefix = prefixes
    }

    pub fn add_module_by_path(&mut self, file_id: FileId, path: &str) -> Option<WorkspaceId> {
        if self.file_module_map.contains_key(&file_id) {
            self.remove(file_id);
        }

        let (module_path, workspace_id) = self.extract_module_path(&path)?;
        let mut module_path = module_path.replace(['\\', '/'], ".");
        if !self.module_replace_vec.is_empty() {
            module_path = self.replace_module_path(&module_path);
        }

        self.add_module_by_module_path(file_id, module_path, workspace_id);
        Some(workspace_id)
    }

    pub fn add_module_by_module_path(
        &mut self,
        file_id: FileId,
        module_path: String,
        workspace_id: WorkspaceId,
    ) -> Option<()> {
        if self.file_module_map.contains_key(&file_id) {
            self.remove(file_id);
        }

        let module_parts: Vec<&str> = module_path.split('.').collect();
        if module_parts.is_empty() {
            return None;
        }

        let mut parent_node_id = self.module_root_id;
        for part in &module_parts {
            // I had to struggle with Rust's ownership rules, making the code look like this.
            let child_id = {
                let parent_node = match self.module_nodes.get_mut(&parent_node_id) {
                    Some(node) => node,
                    None => return None,
                };
                let node_id = parent_node.children.get(*part);
                match node_id {
                    Some(id) => *id,
                    None => {
                        let new_id = ModuleNodeId {
                            id: self.id_counter,
                        };
                        parent_node.children.insert(part.to_string(), new_id);
                        new_id
                    }
                }
            };
            if !self.module_nodes.contains_key(&child_id) {
                let new_node = ModuleNode {
                    children: HashMap::new(),
                    file_ids: Vec::new(),
                    parent: Some(parent_node_id),
                };

                self.module_nodes.insert(child_id, new_node);
                self.id_counter += 1;
            }

            parent_node_id = child_id;
        }

        let node = match self.module_nodes.get_mut(&parent_node_id) {
            Some(node) => node,
            None => return None,
        };

        node.file_ids.push(file_id);
        let module_name = match module_parts.last() {
            Some(name) => name.to_string(),
            None => return None,
        };
        let module_info = ModuleInfo {
            file_id,
            full_module_name: module_parts.join("."),
            name: module_name.clone(),
            module_id: parent_node_id,
            visible: true,
            export_type: None,
            version_conds: None,
            workspace_id,
            semantic_id: None,
            is_meta: false,
            is_kg_required: false,
        };

        self.file_module_map.insert(file_id, module_info);
        if self.fuzzy_search {
            self.module_name_to_file_ids
                .entry(module_name)
                .or_insert(Vec::new())
                .push(file_id);
        }

        Some(())
    }

    pub fn get_module(&self, file_id: FileId) -> Option<&ModuleInfo> {
        self.file_module_map.get(&file_id)
    }

    pub fn get_module_mut(&mut self, file_id: FileId) -> Option<&mut ModuleInfo> {
        self.file_module_map.get_mut(&file_id)
    }

    pub fn set_module_visibility(&mut self, file_id: FileId, visible: bool) {
        if let Some(module_info) = self.file_module_map.get_mut(&file_id) {
            module_info.visible = visible;
        }
    }

    pub fn set_module_version_conds(
        &mut self,
        file_id: FileId,
        version_conds: Vec<LuaVersionCondition>,
    ) {
        if let Some(module_info) = self.file_module_map.get_mut(&file_id) {
            module_info.version_conds = Some(Box::new(version_conds));
        }
    }

    pub fn find_module(&self, module_path: &str) -> Option<&ModuleInfo> {
        let module_path = module_path.replace(['\\', '/'], ".");
        let module_parts: Vec<&str> = module_path.split('.').collect();
        if module_parts.is_empty() {
            return None;
        }

        let result = self.exact_find_module(&module_parts);
        if result.is_some() {
            return result;
        }

        if self.fuzzy_search {
            let last_name = match module_parts.last() {
                Some(name) => name,
                None => return None,
            };

            return self.fuzzy_find_module(&module_path, last_name);
        }

        None
    }

    fn exact_find_module(&self, module_parts: &Vec<&str>) -> Option<&ModuleInfo> {
        let mut parent_node_id = self.module_root_id;
        for part in module_parts {
            let parent_node = self.module_nodes.get(&parent_node_id)?;
            let child_id = match parent_node.children.get(*part) {
                Some(id) => *id,
                None => return None,
            };
            parent_node_id = child_id;
        }

        let node = self.module_nodes.get(&parent_node_id)?;
        let file_id = node.file_ids.first()?;
        self.file_module_map.get(file_id)
    }

    fn fuzzy_find_module(&self, module_path: &str, last_name: &str) -> Option<&ModuleInfo> {
        let file_ids = self.module_name_to_file_ids.get(last_name)?;
        if file_ids.len() == 1 {
            return self.file_module_map.get(&file_ids[0]);
        }

        // find the first matched module
        for file_id in file_ids {
            let module_info = self.file_module_map.get(file_id)?;
            if module_info.full_module_name.ends_with(module_path) {
                return Some(module_info);
            }
        }

        None
    }

    /// Find a module node by module path.
    /// The module path is a string separated by dots.
    /// For example, "a.b.c" represents the module "c" in the module "b" in the module "a".
    pub fn find_module_node(&self, module_path: &str) -> Option<&ModuleNode> {
        if module_path.is_empty() {
            return self.module_nodes.get(&self.module_root_id);
        }

        let module_path = module_path.replace(['\\', '/'], ".");
        let module_parts: Vec<&str> = module_path.split('.').collect();
        if module_parts.is_empty() {
            return None;
        }

        let mut parent_node_id = self.module_root_id;
        for part in &module_parts {
            let parent_node = self.module_nodes.get(&parent_node_id)?;
            let child_id = parent_node.children.get(*part)?;
            parent_node_id = *child_id;
        }

        self.module_nodes.get(&parent_node_id)
    }

    pub fn get_module_node(&self, module_id: &ModuleNodeId) -> Option<&ModuleNode> {
        self.module_nodes.get(module_id)
    }

    pub fn get_module_infos(&self) -> Vec<&ModuleInfo> {
        self.file_module_map.values().collect()
    }

    pub fn extract_module_path(&self, path: &str) -> Option<(String, WorkspaceId)> {
        let path = Path::new(path);
        let mut matched_module_path_and_root: Option<(String, WorkspaceId, PathBuf)> = None;
        for workspace in &self.workspaces {
            if let Ok(relative_path) = path.strip_prefix(&workspace.root) {
                let relative_path_str = relative_path.to_str().unwrap_or("");
                let module_path = self.match_pattern(relative_path_str);
                if let Some(module_path) = module_path {
                    if matched_module_path_and_root.is_none() {
                        matched_module_path_and_root =
                            Some((module_path, workspace.id, workspace.root.clone()));
                    } else {
                        let (matched, matched_workspace_id) =
                            match matched_module_path_and_root.as_ref() {
                                Some((matched, id, _)) => (matched, id),
                                None => continue,
                            };
                        if module_path.len() < matched.len() {
                            // Libraries could be in a subdirectory of the main workspace
                            // In case of a conflict, we prioritise the non-main workspace ID
                            let workspace_id = if workspace.id.is_main() {
                                *matched_workspace_id
                            } else {
                                workspace.id
                            };
                            matched_module_path_and_root =
                                Some((module_path, workspace_id, workspace.root.clone()));
                        }
                    }
                }
            }
        }

        matched_module_path_and_root.map(|(path, workspace_id, root)| {
            let root = root.to_owned();
            for it in self.workspace_module_prefix.iter() {
                if root.to_str().map(|root| it.0.is_match(root)).unwrap_or(false) {
                    if let Some(path) = PathBuf::from(&it.1).join(&path).to_str().to_owned() {
                        return (path.to_owned(), workspace_id);
                    }
                }
            }
            (path, workspace_id)
        })
    }

    fn replace_module_path(&self, module_path: &str) -> String {
        for (key, value) in &self.module_replace_vec {
            return key.replace_all(&module_path, value).to_string();
        }

        module_path.to_string()
    }

    pub fn match_pattern(&self, path: &str) -> Option<String> {
        for pattern in &self.module_patterns {
            if let Some(captures) = pattern.captures(path) {
                if let Some(matched) = captures.get(1) {
                    return Some(matched.as_str().to_string());
                }
            }
        }

        None
    }

    pub fn add_workspace_root(&mut self, root: PathBuf, workspace_id: WorkspaceId) {
        if !self.workspaces.iter().any(|w| w.root == root) {
            self.workspaces.push(Workspace::new(root, workspace_id));
        }
    }

    #[allow(unused)]
    pub fn remove_workspace_root(&mut self, root: &Path) {
        self.workspaces.retain(|r| r.root != root);
    }

    pub fn update_config(&mut self, config: Arc<Emmyrc>) {
        let mut extension_names = Vec::new();

        for extension in &config.runtime.extensions {
            if extension.starts_with(".") {
                extension_names.push(extension[1..].to_string());
            } else if extension.starts_with("*.") {
                extension_names.push(extension[2..].to_string());
            } else {
                extension_names.push(extension.clone());
            }
        }

        if !extension_names.contains(&"lua".to_string()) {
            extension_names.push("lua".to_string());
        }

        let mut patterns = Vec::new();
        for extension in &extension_names {
            patterns.push(format!("?.{}", extension));
        }

        let require_pattern = config.runtime.require_pattern.clone();
        if require_pattern.is_empty() {
            // add default require pattern
            for extension in &extension_names {
                patterns.push(format!("?/init.{}", extension));
            }
        } else {
            patterns.extend(require_pattern);
        }

        self.set_module_extract_patterns(patterns);
        self.set_module_replace_patterns(
            config
                .workspace
                .module_map
                .iter()
                .map(|m| (m.pattern.clone(), m.replace.clone()))
                .collect(),
        );
        self.set_workspace_module_prefix(
            config
                .workspace
                .workspace_prefix_map
                .iter()
                .map(|it| {
                    Regex::new(&it.path).map(|re| (re, it.prefix.clone()))
                })
                .flatten()
                .collect(),
        );

        self.default_kg_require = config.kg_custom.default_kg_require;
        self.force_raw_require_patterns.clear();
        for it in config.kg_custom.force_raw_require_re.iter() {
            if let Ok(re) = Regex::new(it) {
                self.force_raw_require_patterns.push(re);
            }
        }

        self.fuzzy_search = !config.strict.require_path;
    }

    pub fn get_std_file_ids(&self) -> Vec<FileId> {
        let mut file_ids = Vec::new();
        for module_info in self.file_module_map.values() {
            if module_info.workspace_id == WorkspaceId::STD {
                file_ids.push(module_info.file_id);
            }
        }

        file_ids
    }

    pub fn is_main(&self, file_id: &FileId) -> bool {
        if let Some(module_info) = self.file_module_map.get(file_id) {
            return module_info.workspace_id == WorkspaceId::MAIN;
        }

        false
    }

    pub fn is_std(&self, file_id: &FileId) -> bool {
        if let Some(module_info) = self.file_module_map.get(file_id) {
            return module_info.workspace_id == WorkspaceId::STD;
        }

        false
    }

    pub fn is_library(&self, file_id: &FileId) -> bool {
        if let Some(module_info) = self.file_module_map.get(file_id) {
            return module_info.workspace_id.is_library();
        }

        false
    }

    pub fn get_main_workspace_file_ids(&self) -> Vec<FileId> {
        let mut file_ids = Vec::new();
        for module_info in self.file_module_map.values() {
            if module_info.workspace_id == WorkspaceId::MAIN {
                file_ids.push(module_info.file_id);
            }
        }

        file_ids
    }

    pub fn get_lib_file_ids(&self) -> Vec<FileId> {
        let mut file_ids = Vec::new();
        for module_info in self.file_module_map.values() {
            if module_info.workspace_id.is_library() {
                file_ids.push(module_info.file_id);
            }
        }

        file_ids
    }

    pub fn set_meta(&mut self, file_id: FileId) {
        if let Some(module_info) = self.file_module_map.get_mut(&file_id) {
            module_info.is_meta = true;
        }
    }

    pub fn is_meta_file(&self, file_id: &FileId) -> bool {
        if let Some(module_info) = self.file_module_map.get(&file_id) {
            return module_info.is_meta;
        }

        false
    }

    pub fn set_kg_required(&mut self, file_id: &FileId) {
        if let Some(module_info) = self.file_module_map.get_mut(&file_id) {
            module_info.is_kg_required = true;
        }
    }

    pub fn is_kg_required(&self, file_id: &FileId) -> bool {
        if let Some(module_info) = self.file_module_map.get(&file_id) {
            self.default_kg_require && !self.force_raw_require_patterns.iter().any(|it| it.is_match(&module_info.full_module_name))
        } else{
            false
        }
    }

    pub fn get_workspace_id(&self, file_id: FileId) -> Option<WorkspaceId> {
        if let Some(module_info) = self.file_module_map.get(&file_id) {
            return Some(module_info.workspace_id);
        }

        None
    }
}

impl LuaIndex for LuaModuleIndex {
    fn remove(&mut self, file_id: FileId) {
        let (mut parent_id, mut child_id) =
            if let Some(module_info) = self.file_module_map.remove(&file_id) {
                let module_id = module_info.module_id;
                let node = match self.module_nodes.get_mut(&module_id) {
                    Some(node) => node,
                    None => return,
                };
                node.file_ids.retain(|id| *id != file_id);
                if node.file_ids.is_empty() && node.children.is_empty() {
                    (node.parent, Some(module_id))
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            };

        if parent_id.is_none() || child_id.is_none() {
            return;
        }

        while let Some(id) = parent_id {
            let child_module_id = match child_id {
                Some(id) => id,
                None => break,
            };
            let node = match self.module_nodes.get_mut(&id) {
                Some(node) => node,
                None => break,
            };
            node.children
                .retain(|_, node_child_idid| *node_child_idid != child_module_id);

            if id == self.module_root_id {
                return;
            }

            if node.file_ids.is_empty() && node.children.is_empty() {
                child_id = Some(id);
                parent_id = node.parent;
                self.module_nodes.remove(&id);
            } else {
                break;
            }
        }

        if !self.module_name_to_file_ids.is_empty() {
            let mut module_name = String::new();
            for (name, file_ids) in &self.module_name_to_file_ids {
                if file_ids.contains(&file_id) {
                    module_name = name.clone();
                    break;
                }
            }

            if !module_name.is_empty() {
                let file_ids = match self.module_name_to_file_ids.get_mut(&module_name) {
                    Some(ids) => ids,
                    None => return,
                };

                file_ids.retain(|id| *id != file_id);
                if file_ids.is_empty() {
                    self.module_name_to_file_ids.remove(&module_name);
                }
            }
        }
    }

    fn clear(&mut self) {
        self.module_nodes.clear();
        self.file_module_map.clear();
        self.module_name_to_file_ids.clear();

        let root_node = ModuleNode::default();
        self.module_nodes.insert(self.module_root_id, root_node);
    }
}
