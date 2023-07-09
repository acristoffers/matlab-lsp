/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;

use crossbeam_channel::Sender;
use lsp_server::Message;
use lsp_types::{InitializeParams, Url};
use tree_sitter::Tree;

// Global state types

#[derive(Debug)]
pub struct SessionState {
    // Misc
    /// Whether the client requested us to shutdown.
    pub client_requested_shutdown: bool,
    /// The path given by the user through command line or env var.
    pub path: Vec<String>,
    /// Channel used to send messages to the client.
    pub sender: Sender<Message>,
    /// The workspace parameters sent by the client.
    pub workspace: InitializeParams,

    // Code states and structures
    /// A list of all files in the workspace+path, even the ones inside namespaces.
    pub files: HashMap<String, ParsedFile>,
    /// A hashmap with all first-level namespaces. Every namespace may contain nested namespaces
    /// and classes.
    pub namespaces: HashMap<String, Namespace>,
    /// A hashmap with all first-level class folders. Every class folder may contain nested
    /// namespaces and classes.
    pub classes: HashMap<String, ClassFolder>,
}

#[derive(Debug)]
pub struct Namespace {
    pub name: String,
    pub files: Vec<String>,
    pub namespaces: HashMap<String, Namespace>,
    pub classes: HashMap<String, ClassFolder>,
}

#[derive(Debug)]
pub struct ClassFolder {
    pub name: String,
    pub files: Vec<String>,
    pub namespaces: HashMap<String, Namespace>,
    pub classes: HashMap<String, ClassFolder>,
}

// File related types

#[derive(Debug)]
pub struct FileTypeFunction {
    pub name: String,
    pub argin: usize,
    pub argout: usize,
    pub vargin: bool,
    pub vargout: bool,
    pub argout_names: Vec<String>,
    pub argin_names: Vec<String>,
    pub vargin_names: Vec<String>,
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
