/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;
use std::path::Path;

use crate::formatter::format;
use crate::utils::read_to_string;
use anyhow::{anyhow, Context, Result};
use log::{error, info};
use lsp_types::Url;
use tree_sitter::{Node, Tree};

#[derive(Debug)]
pub struct FileTypeFunction {
    name: String,
    argin: usize,
    argout: usize,
    vargin: bool,
    vargout: bool,
    argout_names: Vec<String>,
    argin_names: Vec<String>,
    vargin_names: Vec<String>,
}

#[derive(Debug)]
pub enum FileType {
    Function(FileTypeFunction),
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
            let tree = tree.clone();
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
                            FileType::Function(self.define_function_type(child)?)
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

    fn define_function_type(&mut self, node: Node) -> Result<FileTypeFunction> {
        let name = if let Some(name) = node.child_by_field_name("name") {
            name.utf8_text(self.contents.as_bytes())?.to_string()
        } else {
            return Err(anyhow!("Could not find class name"));
        };
        let mut cursor = node.walk();
        let mut argout: usize = 0;
        let mut vargout = false;
        let mut argout_names = vec![];
        if let Some(output) = node
            .named_children(&mut cursor)
            .find(|c| c.kind() == "function_output")
        {
            if let Some(args) = output.child(0) {
                if args.kind() == "identifier" {
                    argout = 1;
                    argout_names.push(args.utf8_text(self.contents.as_bytes())?.into());
                } else {
                    argout = args.named_child_count();
                    let mut cursor2 = args.walk();
                    for arg_name in args
                        .named_children(&mut cursor2)
                        .filter(|c| c.kind() == "identifier")
                        .filter_map(|c| c.utf8_text(self.contents.as_bytes()).ok())
                        .map(String::from)
                    {
                        if arg_name == "varargout" {
                            vargout = true;
                        } else {
                            argout_names.push(arg_name);
                        }
                    }
                    if vargout {
                        argout -= 1;
                    }
                }
            }
        }
        let mut argin: usize = 0;
        let mut vargin = false;
        let mut argin_names = vec![];
        let mut vargin_names = vec![];
        if let Some(inputs) = node
            .named_children(&mut cursor)
            .find(|c| c.kind() == "function_arguments")
        {
            argin = inputs.named_child_count();
            let mut cursor2 = node.walk();
            let mut cursor3 = node.walk();
            let mut cursor4 = node.walk();
            for arg_name in inputs
                .named_children(&mut cursor2)
                .filter_map(|c| c.utf8_text(self.contents.as_bytes()).ok())
                .map(String::from)
            {
                argin_names.push(arg_name);
            }
            let mut optional_arguments = HashMap::new();
            for argument in node
                .named_children(&mut cursor2)
                .filter(|c| c.kind() == "arguments_statement")
            {
                if let Some(attributes) = argument
                    .named_children(&mut cursor3)
                    .find(|c| c.kind() == "attributes")
                {
                    if attributes
                        .named_children(&mut cursor4)
                        .filter_map(|c| c.utf8_text(self.contents.as_bytes()).ok())
                        .any(|c| c == "Output")
                    {
                        continue;
                    }
                }
                for property in argument
                    .named_children(&mut cursor3)
                    .filter_map(|c| c.child_by_field_name("name"))
                    .filter(|c| c.kind() == "property_name")
                {
                    let arg_name = property
                        .named_child(0)
                        .unwrap()
                        .utf8_text(self.contents.as_bytes())?
                        .to_string();
                    argin_names.retain(|e| *e != arg_name);
                    optional_arguments.insert(arg_name, ());
                    let opt_arg_name = property
                        .named_child(1)
                        .unwrap()
                        .utf8_text(self.contents.as_bytes())?
                        .to_string();
                    vargin_names.push(opt_arg_name);
                }
            }
            let vargin_count = optional_arguments.keys().count();
            vargin = vargin_count > 0;
            argin -= vargin_count;
        }
        let function = FileTypeFunction {
            name,
            argin,
            argout,
            vargin,
            vargout,
            argout_names,
            argin_names,
            vargin_names,
        };
        Ok(function)
    }
}
