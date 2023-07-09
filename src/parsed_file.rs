/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::formatter::format;
use crate::types::{ClassDefinition, FunctionDefinition, Workspace};
pub use crate::types::{FileType, FunctionSignature, ParsedFile};
use crate::utils::{function_signature, lock_mutex, read_to_string, SessionStateArc};
use anyhow::{anyhow, Context, Result};
use log::{debug, error, info};
use lsp_types::Url;

impl ParsedFile {
    pub fn parse(&mut self) -> Result<()> {
        info!("Parsing {}", self.path.as_str());
        self.load_contents()?;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(tree_sitter_matlab::language())
            .with_context(|| "Could not set Tree-Sitter language")?;
        let tree = parser
            .parse(&self.contents, None)
            .ok_or_else(|| anyhow!("Could not parse file."))?;
        self.tree = Some(tree);
        info!("Parsed {}", self.path.as_str());
        Ok(())
    }

    pub fn format(&mut self) -> Option<String> {
        if let Some(tree) = &self.tree {
            if tree.root_node().has_error() {
                debug!("Cannot format, has errors.");
                return None;
            }
        } else {
            debug!("Cannot format, has no tree.");
            return None;
        }
        if let Err(err) = self.load_contents() {
            error!("Error loading contents: {err}");
            return None;
        }
        let result = format((self.contents.clone() + "\n").as_str()).ok();
        self.dump_contents();
        result
    }

    pub fn parse_file(path: String) -> Result<ParsedFile> {
        info!("Reading file from path {}", path);
        let file_uri = "file://".to_string() + path.as_str();
        let mut file = std::fs::File::open(&path)?;
        let code = read_to_string(&mut file, None)?.0 + "\n";
        let path_p = Path::new(&path);
        let mut scope = String::new();
        for segment in path_p.iter() {
            let segment = segment.to_string_lossy().to_string();
            if segment.starts_with('+') || segment.starts_with('@') {
                if !scope.is_empty() {
                    scope += "/";
                }
                scope += segment.as_str();
            }
        }
        let file = Url::parse(file_uri.as_str())?;
        let file_name: String = if let Some(segs) = file.path_segments() {
            if let Some(name) = segs.filter(|c| !c.is_empty()).last() {
                if let Some(name) = name.to_string().strip_suffix(".m") {
                    name.into()
                } else {
                    name.into()
                }
            } else {
                "".into()
            }
        } else {
            "".into()
        };
        let mut parsed_file = ParsedFile {
            contents: code,
            path,
            name: file_name,
            file_type: FileType::MScript,
            in_classfolder: None,
            in_namespace: None,
            open: false,
            tree: None,
            workspace: Workspace::default(),
        };
        parsed_file.parse()?;
        Ok(parsed_file)
    }

    pub fn load_contents(&mut self) -> Result<()> {
        if !self.open {
            let mut file = std::fs::File::open(self.path.clone())?;
            self.contents = read_to_string(&mut file, None)?.0;
        }
        Ok(())
    }

    pub fn dump_contents(&mut self) {
        if !self.open {
            self.contents = "".into();
        }
    }

    pub fn define_type(
        parsed_file: Arc<Mutex<ParsedFile>>,
        namespace: String,
        state: SessionStateArc,
    ) -> Result<()> {
        debug!("Defining the type of a file.");
        let mut state = lock_mutex(&state)?;
        let mut parsed_file_lock = lock_mutex(&parsed_file)?;
        debug!("Locked the mutexes. File: {}", parsed_file_lock.path);
        if let Some(tree) = &parsed_file_lock.tree {
            debug!("File has a tree, continuing...");
            let tree = tree.clone();
            let root = tree.root_node();
            let mut cursor = root.walk();
            if root.child_count() > 0 {
                // Finds the first non-comment line.
                if let Some(child) = root.named_children(&mut cursor).find(|c| !c.is_extra()) {
                    let file_type = match child.kind() {
                        "class_definition" => {
                            debug!("It's a class definition. Parsing.");
                            if let Some(name) = child.child_by_field_name("name") {
                                let class_name = name
                                    .utf8_text(parsed_file_lock.contents.as_bytes())?
                                    .to_string();
                                let qualified_name = if !namespace.is_empty() {
                                    namespace + "." + class_name.as_str()
                                } else {
                                    class_name.clone()
                                };
                                let class_def = ClassDefinition {
                                    parsed_file: Arc::clone(&parsed_file),
                                    loc: name.range().into(),
                                    name: class_name,
                                    path: qualified_name.clone(),
                                };
                                let class_def = Arc::new(Mutex::new(class_def));
                                state
                                    .workspace
                                    .classes
                                    .insert(qualified_name, Arc::clone(&class_def));
                                FileType::Class(class_def)
                            } else {
                                return Err(anyhow!("Could not find class name"));
                            }
                        }
                        "function_definition" => {
                            debug!("It's a function definition. Parsing.");
                            let fn_sig = function_signature(&parsed_file_lock, child)?;
                            debug!("Got signature for {}", fn_sig.name);
                            let qualified_name = if !namespace.is_empty() {
                                namespace + "." + fn_sig.name.as_str()
                            } else {
                                fn_sig.name.clone()
                            };
                            let fn_def = FunctionDefinition {
                                loc: fn_sig.name_range,
                                parsed_file: Arc::clone(&parsed_file),
                                name: fn_sig.name.clone(),
                                signature: fn_sig,
                                path: qualified_name.clone(),
                            };
                            let fn_def = Arc::new(Mutex::new(fn_def));
                            debug!("Inserting function {qualified_name} into state.");
                            state
                                .workspace
                                .functions
                                .insert(qualified_name, Arc::clone(&fn_def));
                            FileType::Function(Arc::clone(&fn_def))
                        }
                        _ => {
                            let qualified_name = if !namespace.is_empty() {
                                namespace + "." + parsed_file_lock.name.as_str()
                            } else {
                                parsed_file_lock.name.clone()
                            };
                            state
                                .workspace
                                .scripts
                                .insert(qualified_name, Arc::clone(&parsed_file));
                            FileType::MScript
                        }
                    };
                    debug!(
                        "Defined {} to be of type {:#?}",
                        parsed_file_lock.path, file_type
                    );
                    parsed_file_lock.file_type = file_type;
                }
            }
            Ok(())
        } else {
            error!("File has no tree: {}", parsed_file_lock.path.as_str());
            Err(anyhow!("File has no tree"))
        }
    }
}
