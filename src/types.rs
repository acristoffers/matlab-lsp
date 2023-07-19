/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;

use anyhow::{anyhow, Context};
use atomic_refcell::{AtomicRefCell, AtomicRefMut};
use crossbeam_channel::Sender;
use lsp_server::Message;
use lsp_types::{InitializeParams, Position};
use tree_sitter::{Point, Tree};

use crate::code_loc;

// Global state types

#[derive(Debug)]
pub struct SessionState {
    // Misc
    /// The path given by the user through command line or env var.
    pub path: Vec<String>,
    /// Channel used to send messages to the client.
    pub sender: Sender<Message>,
    /// The workspace parameters sent by the client.
    pub workspace_params: InitializeParams,

    // Worker thread management
    /// Whether the client requested us to shutdown.
    pub client_requested_shutdown: bool,
    /// Whether open files should be rescanned
    pub rescan_open_files: bool,
    /// Whether all files should be rescanned
    pub rescan_all_files: bool,

    // Code states and structures
    /// A list of all files in the workspace+path, even the ones inside namespaces.
    pub files: HashMap<String, Arc<AtomicRefCell<ParsedFile>>>,
    // The path workspace.
    pub workspace: Workspace,

    /// Id of last sent request
    pub request_id: i32,
}

#[derive(Debug, Clone, Default)]
pub struct Namespace {
    pub name: String,
    pub path: String,
    pub files: Vec<Arc<AtomicRefCell<ParsedFile>>>,
    pub namespaces: HashMap<String, Arc<AtomicRefCell<Namespace>>>,
    pub classfolders: HashMap<String, Arc<AtomicRefCell<ClassFolder>>>,
    pub functions: HashMap<String, Arc<AtomicRefCell<FunctionDefinition>>>,
    pub classes: HashMap<String, Arc<AtomicRefCell<ClassDefinition>>>,
}

#[derive(Debug, Clone, Default)]
pub struct ClassFolder {
    pub name: String,
    pub path: String,
    pub files: Vec<Arc<AtomicRefCell<ParsedFile>>>,
    pub methods: Vec<Arc<AtomicRefCell<FunctionDefinition>>>,
}

// File related types

#[derive(Debug, Clone, Default)]
pub struct FunctionSignature {
    pub name_range: Range,
    pub name: String,
    pub argin: usize,
    pub argout: usize,
    pub vargin: bool,
    pub vargout: bool,
    pub argout_names: Vec<String>,
    pub argin_names: Vec<String>,
    pub vargin_names: Vec<String>,
    pub documentation: String,
    pub range: Range,
}

#[derive(Debug, Clone, Default)]
pub enum FileType {
    Function(Arc<AtomicRefCell<FunctionDefinition>>),
    Class(Arc<AtomicRefCell<ClassDefinition>>),
    #[default]
    MScript,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedFile {
    /// The file contents as a string. If the file is not open, this will be kept empty.
    pub contents: String,
    /// The file url as a Unix file path.
    pub path: String,
    /// File name without the .m extension.
    pub name: String,
    /// Is this a script, function or class?.
    pub file_type: FileType,
    /// Whether this file is inside a @folder.
    pub in_classfolder: Option<Arc<AtomicRefCell<ClassFolder>>>,
    /// Whether this file is inside a +folder.
    pub in_namespace: Option<Arc<AtomicRefCell<Namespace>>>,
    /// Whether the file is currently open in the editor.
    pub open: bool,
    /// The file's parsed tree.
    pub tree: Option<Tree>,
    /// Definitions inside this file
    pub workspace: Workspace,
}

// Analysis related types

#[derive(Debug, Clone, Default)]
pub struct FunctionDefinition {
    pub loc: Range,
    pub name: String,
    pub parsed_file: Arc<AtomicRefCell<ParsedFile>>,
    pub path: String,
    pub signature: FunctionSignature,
}

#[derive(Debug, Clone, Default)]
pub struct ClassDefinition {
    pub parsed_file: Arc<AtomicRefCell<ParsedFile>>,
    pub loc: Range,
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Default)]
pub struct VariableDefinition {
    pub loc: Range,
    pub name: String,
    pub cleared: bool,
    pub is_parameter: bool,
}

#[derive(Debug, Clone, Default)]
pub enum ReferenceTarget {
    Class(Arc<AtomicRefCell<ClassDefinition>>),
    ClassFolder(Arc<AtomicRefCell<ClassFolder>>),
    Function(Arc<AtomicRefCell<FunctionDefinition>>),
    Namespace(Arc<AtomicRefCell<Namespace>>),
    Script(Arc<AtomicRefCell<ParsedFile>>),
    #[default]
    UnknownVariable,
    UnknownFunction,
    Variable(Arc<AtomicRefCell<VariableDefinition>>),
}

#[derive(Debug, Clone, Default)]
pub struct Reference {
    pub loc: Range,
    pub name: String,
    pub target: ReferenceTarget,
}

#[derive(Debug, Clone, Default)]
pub struct Workspace {
    pub classes: HashMap<String, Arc<AtomicRefCell<ClassDefinition>>>,
    pub classfolders: HashMap<String, Arc<AtomicRefCell<ClassFolder>>>,
    pub functions: HashMap<String, Arc<AtomicRefCell<FunctionDefinition>>>,
    pub namespaces: HashMap<String, Arc<AtomicRefCell<Namespace>>>,
    pub references: Vec<Arc<AtomicRefCell<Reference>>>,
    pub scripts: HashMap<String, Arc<AtomicRefCell<ParsedFile>>>,
    pub variables: Vec<Arc<AtomicRefCell<VariableDefinition>>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Range {
    pub start: Point,
    pub end: Point,
}

impl From<tree_sitter::Range> for Range {
    fn from(value: tree_sitter::Range) -> Self {
        Range {
            start: value.start_point,
            end: value.end_point,
        }
    }
}

impl From<Range> for tree_sitter::Range {
    fn from(value: Range) -> Self {
        tree_sitter::Range {
            start_byte: 0,
            end_byte: 0,
            start_point: value.start,
            end_point: value.end,
        }
    }
}

impl From<lsp_types::Range> for Range {
    fn from(value: lsp_types::Range) -> Self {
        Range {
            start: value.start.to_point(),
            end: value.end.to_point(),
        }
    }
}

impl From<Range> for lsp_types::Range {
    fn from(value: Range) -> Self {
        lsp_types::Range {
            start: value.start.to_position(),
            end: value.end.to_position(),
        }
    }
}

impl Display for Range {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Range {{({}, {}), ({}, {})}}",
            self.start.row, self.start.column, self.end.row, self.end.column
        )
    }
}

impl Range {
    pub fn fully_contains(&self, other: Range) -> bool {
        self.contains(other.start) && self.contains(other.end)
    }

    pub fn contains(&self, other: Point) -> bool {
        self.start.row < other.row && other.row < self.end.row
            || (self.start.row == other.row || self.end.row == other.row)
                && self.start.column <= other.column
                && other.column <= self.end.column
    }

    pub fn find_bytes(&self, parsed_file: &AtomicRefMut<'_, ParsedFile>) -> tree_sitter::Range {
        let mut byte = 0;
        let mut row = 0;
        let mut col = 0;
        let mut start_byte = 0;
        let mut end_byte = 0;
        let mut chars = parsed_file.contents.chars();
        if let Some(tree) = &parsed_file.tree {
            if let Some(node) = tree
                .root_node()
                .descendant_for_point_range(self.start, self.end)
            {
                byte = node.start_byte();
                row = node.start_position().row;
                col = node.start_position().column;
                chars = parsed_file.contents[byte..].chars();
            }
        }
        loop {
            if row == self.start.row && col == self.start.column {
                start_byte = byte;
            }
            if row == self.end.row && col == self.end.column {
                end_byte = byte;
                break;
            }
            if let Some(c) = chars.next() {
                byte += c.len_utf8();
                col += 1;
                if c == '\n' {
                    row += 1;
                    col = 0;
                }
            } else {
                break;
            }
        }
        let mut tree_range: tree_sitter::Range = self.to_owned().into();
        tree_range.start_byte = start_byte;
        tree_range.end_byte = end_byte;
        tree_range
    }
}

pub trait PosToPoint {
    fn to_point(&self) -> Point;
}

impl PosToPoint for Position {
    fn to_point(&self) -> Point {
        Point {
            row: self
                .line
                .try_into()
                .context(code_loc!("Error converting number."))
                .unwrap(),
            column: self
                .character
                .try_into()
                .context(code_loc!("Error converting number."))
                .unwrap(),
        }
    }
}

pub trait PointToPos {
    fn to_position(&self) -> Position;
}

impl PointToPos for Point {
    fn to_position(&self) -> Position {
        Position::new(
            self.row
                .try_into()
                .context(code_loc!("Error converting number."))
                .unwrap(),
            self.column
                .try_into()
                .context(code_loc!("Error converting number."))
                .unwrap(),
        )
    }
}
