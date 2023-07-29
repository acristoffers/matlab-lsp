/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use atomic_refcell::AtomicRefCell;
use lsp_server::{Message, Notification, Request, Response};
use tree_sitter::{Point, Tree};

//////////////////////////////////////////////////////////////////////////////
//                                                                          //
//                             Message Passing                              //
//                                                                          //
//////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone)]
pub enum SenderThread {
    Main,
    Dispatcher,
    Handler,
    BackgroundWorker,
}

#[derive(Debug, Clone)]
pub enum DBOperation {
    Get,
    Set,
    Delete,
    Fetch,
}

#[derive(Debug, Clone)]
pub enum DBTarget {
    FunctionDefinition,
    Package,
    ParsedFile,
    RequestID,
    Script,
}

#[derive(Debug, Clone)]
pub enum DBArgument {
    ParsedFile(Arc<ParsedFile>),
    ParsedFiles(HashMap<String, Arc<ParsedFile>>),
    Packages(Vec<String>),
    FunctionDefinition(Arc<FunctionDefinition>),
    FunctionDefinitions(HashMap<String, Arc<FunctionDefinition>>),
    String(String),
    Integer(i32),
    NotFound,
}

#[derive(Debug, Clone)]
pub struct DBRequest {
    pub operation: DBOperation,
    pub target: DBTarget,
    pub argument: DBArgument,
}

#[derive(Debug, Clone)]
pub enum MessagePayload {
    InitPath((Vec<Arc<ParsedFile>>, Vec<Arc<FunctionDefinition>>)),
    LspMessage(Message),
    DB(DBRequest),
    ScanPath(Vec<String>),
    ScanWorkspace(Vec<String>),
    ScanOpen,
    Done,
    Exit,
}

#[derive(Debug, Clone)]
pub struct ThreadMessage {
    pub sender: SenderThread,
    pub payload: MessagePayload,
}

//////////////////////////////////////////////////////////////////////////////
//                                                                          //
//                               Server State                               //
//                                                                          //
//////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone)]
pub struct State {
    /// Path of libraries, given as an argument or ENV var.
    pub lib_path: Vec<String>,
    /// Path of the current workspace.
    pub ws_path: Vec<String>,

    /// Request queue, of items waiting to be processed.
    pub requests_queue: VecDeque<Request>,
    /// Request queue, of items waiting to be processed.
    pub notifications_queue: VecDeque<Notification>,
    /// Request queue, of items waiting to be processed.
    pub responses_queue: VecDeque<Response>,
    /// Handler general queue
    pub handler_queue: VecDeque<ThreadMessage>,
    /// Background Worker queue
    pub bw_queue: VecDeque<ThreadMessage>,

    /// Whether the Handler thread is idle.
    pub handler_idle: bool,
    /// Whether the Background Worker thread is idle.
    pub bw_idle: bool,
    /// The id of last sent request.
    pub request_id: i32,

    /// Map of parsed files. The key is the file's path.
    pub parsed_files: HashMap<String, Arc<ParsedFile>>,
    /// Global Workspace
    pub workspace: Workspace,
}

//////////////////////////////////////////////////////////////////////////////
//                                                                          //
//                               File Symbols                               //
//                                                                          //
//////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Clone)]
pub struct ParsedFile {
    /// The file contents as a string. If the file is not open, this will be kept empty.
    pub contents: String,
    /// File name without the .m extension.
    pub name: String,
    /// The file url as a Unix file path.
    pub path: String,
    /// Whether the file is currently open in the editor.
    pub open: bool,
    /// The file's parsed tree.
    pub tree: Tree,
    /// The time this object was created
    pub timestamp: Instant,
    /// The package this file is in (or empty if none)
    pub package: String,
    /// Whether this file is a script
    pub is_script: bool,
    /// Workspace
    pub workspace: Workspace,
}

#[derive(Debug, Clone, Default)]
pub struct FunctionSignature {
    /// Range of the function's name.
    pub name_range: Range,
    /// Function's name.
    pub name: String,
    /// Number of required input arguments.
    pub argin: usize,
    /// Number of required output arguments.
    pub argout: usize,
    /// Whether variable input arguments are accepted.
    pub vargin: bool,
    /// Whether variable output arguments are accepted.
    pub vargout: bool,
    /// Name of output arguments.
    pub argout_names: Vec<String>,
    /// Name of input arguments.
    pub argin_names: Vec<String>,
    /// Name of variable argument names.
    pub vargin_names: Vec<String>,
    /// Function documentation.
    pub documentation: String,
    /// Range of the entire function.
    pub range: Range,
}

#[derive(Debug, Clone, Default)]
pub struct FunctionDefinition {
    /// Location in the file of the whole function definition.
    pub loc: Range,
    /// Name of the function (without namespace).
    pub name: String,
    /// Path of the file this function is in.
    pub path: String,
    /// Function signature.
    pub signature: FunctionSignature,
    /// Package this function is in (or empty if not)
    pub package: String,
}

#[derive(Debug, Clone, Default)]
pub struct VariableDefinition {
    pub loc: Range,
    pub name: String,
    pub cleared: usize,
    pub is_parameter: bool,
    pub is_global: bool,
}

#[derive(Debug, Clone, Default)]
pub enum ReferenceTarget {
    #[default]
    UnknownVariable,
    UnknownFunction,
    Namespace(String),
    Script(String),
    Function(AtomicRefCell<FunctionDefinition>),
    Variable(AtomicRefCell<VariableDefinition>),
}

#[derive(Debug, Clone, Default)]
pub struct Reference {
    pub loc: Range,
    pub name: String,
    pub target: ReferenceTarget,
}

#[derive(Debug, Clone, Default)]
pub struct Workspace {
    /// Map of qualified function name to function definitions
    pub functions: HashMap<String, Arc<FunctionDefinition>>,
    /// Packages
    pub packages: Vec<String>,
    /// Reference
    pub references: Vec<AtomicRefCell<Reference>>,
    /// Variables
    pub variables: Vec<AtomicRefCell<VariableDefinition>>,
}

//////////////////////////////////////////////////////////////////////////////
//                                                                          //
//                                Utilities                                 //
//                                                                          //
//////////////////////////////////////////////////////////////////////////////

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Range {
    pub start: Point,
    pub end: Point,
}
