/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::path::Path;

use crate::formatter::format;
use crate::utils::read_to_string;
use anyhow::{anyhow, Context, Result};
use log::{error, info};
use lsp_types::Url;
use tree_sitter::Tree;

#[derive(Debug)]
pub enum FileType {
    Function(String),
    Class(String),
    MScript,
}

#[derive(Debug)]
pub struct ParsedFile {
    /// The file contents as a string. If the file is not open, this will be kept empty unless it
    /// is being operated on.
    pub contents: String,
    /// The file URI. Kept as this so the editor can send whatever. However, most functionality of
    /// this server requires a file:// protocol.
    pub file: Url,
    /// Is this a script, function or class?.
    pub file_type: FileType,
    /// Whether this file is inside a @folder.
    pub in_classfolder: bool,
    /// Whether this file is inside a +folder.
    pub in_namespace: bool,
    /// Whether the file is currently open in the editor.
    pub open: bool,
    /// The scope of this file, when it is inside namespaces/class folders.
    /// For example: +lib or +lib/+cvx or @myclass
    pub scope: String,
    /// The file's parsed tree.
    pub tree: Option<Tree>,
}

impl ParsedFile {
    pub fn parse(&mut self) -> Result<()> {
        info!("Parsing {}", self.file.as_str());
        self.load_contents()?;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(tree_sitter_matlab::language())
            .with_context(|| "Could not set Tree-Sitter language")?;
        let tree = parser
            .parse(&self.contents, None)
            .ok_or_else(|| anyhow!("Could not parse file."))?;
        self.tree = Some(tree);
        self.define_type()?;
        self.dump_contents();
        info!("Parsed {}", self.file.as_str());
        Ok(())
    }

    pub fn format(&mut self) -> Option<String> {
        if let Some(tree) = &self.tree {
            if tree.root_node().has_error() {
                return None;
            }
        } else {
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
        let mut parsed_file = ParsedFile {
            contents: code,
            file: Url::parse(file_uri.as_str())?,
            tree: None,
            open: false,
            file_type: FileType::MScript,
            in_classfolder: path.contains('@'),
            in_namespace: path.contains('+'),
            scope,
        };
        parsed_file.parse()?;
        Ok(parsed_file)
    }

    fn load_contents(&mut self) -> Result<()> {
        if !self.open {
            if self.file.scheme() != "file" {
                return Err(anyhow!("File is neither open nor in file system"));
            }
            let mut file = std::fs::File::open(self.file.path())?;
            self.contents = read_to_string(&mut file, None)?.0;
        }
        Ok(())
    }

    fn dump_contents(&mut self) {
        if !self.open {
            self.contents = "".into();
        }
    }

    fn define_type(&mut self) -> Result<()> {
        if let Some(tree) = &self.tree {
            let root = tree.root_node();
            let mut cursor = root.walk();
            if root.child_count() > 0 {
                if let Some(child) = root.named_children(&mut cursor).find(|c| !c.is_extra()) {
                    self.file_type = match child.kind() {
                        "class_definition" => {
                            if let Some(name) = child.child_by_field_name("name") {
                                FileType::Class(name.utf8_text(self.contents.as_bytes())?.into())
                            } else {
                                return Err(anyhow!("Could not find class name"));
                            }
                        }
                        "function_definition" => {
                            if let Some(name) = child.child_by_field_name("name") {
                                FileType::Function(name.utf8_text(self.contents.as_bytes())?.into())
                            } else {
                                return Err(anyhow!("Could not find class name"));
                            }
                        }
                        _ => FileType::MScript,
                    }
                }
            }
            Ok(())
        } else {
            error!("File has no tree: {}", self.file.as_str());
            Err(anyhow!("File has no tree"))
        }
    }
}
