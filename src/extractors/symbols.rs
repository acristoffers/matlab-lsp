/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::code_loc;
use crate::extractors::fast::{function_signature, public_function};
use crate::threads::db::{
    db_fetch_functions, db_get_function, db_get_package, db_get_script, db_set_function,
};
use crate::types::{
    FunctionDefinition, ParsedFile, Range, Reference, ReferenceTarget, SenderThread, ThreadMessage,
    VariableDefinition, Workspace,
};
use anyhow::{anyhow, Result};
use atomic_refcell::AtomicRefCell;
use crossbeam_channel::{Receiver, Sender};
use itertools::Itertools;
use log::{debug, error, info};
use regex::Regex;
use tree_sitter::{Node, Point, Query, QueryCursor};

pub fn extract_symbols(
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    thread: SenderThread,
    parsed_file: Arc<ParsedFile>,
) -> Result<Arc<ParsedFile>> {
    let mut pf_mr = parsed_file.as_ref().clone();
    info!("Analyzing {}", pf_mr.path);
    if pf_mr.contents.is_empty() {
        pf_mr.load_contents()?;
    }
    let scm = include_str!("../queries/defref.scm");
    let query = Query::new(tree_sitter_matlab::language(), scm)?;
    let query_captures: HashMap<u32, String> = query
        .capture_names()
        .iter()
        .flat_map(|n| query.capture_index_for_name(n).map(|i| (i, n.clone())))
        .collect();
    let mut cursor = QueryCursor::new();
    let tree = pf_mr.tree.clone();
    let node = tree.root_node();
    let mut captures: Vec<(String, Node)> = cursor
        .captures(&query, node, pf_mr.contents.as_bytes())
        .map(|(c, _)| c)
        .flat_map(|c| c.captures)
        .flat_map(|c| -> Result<(String, Node)> {
            let capture_name = query_captures
                .get(&c.index)
                .ok_or(code_loc!("Not capture for index."))?
                .clone();
            let node = c.node;
            Ok((capture_name, node))
        })
        .collect();
    captures.sort_by(|(_, n1), (_, n2)| n1.start_byte().cmp(&n2.start_byte()));
    let ws = analyze_impl(
        sender.clone(),
        receiver.clone(),
        thread,
        &captures,
        &mut pf_mr,
    )?;
    pf_mr.workspace = ws;
    pf_mr.dump_contents();
    info!("Analysis finished: {}", pf_mr.path.as_str());
    pf_mr.timestamp = Instant::now();
    Ok(Arc::new(pf_mr))
}

fn analyze_impl(
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    thread: SenderThread,
    captures: &[(String, Node)],
    parsed_file: &mut ParsedFile,
) -> Result<Workspace> {
    let mut workspace = Workspace::default();
    let mut functions: HashMap<usize, (Node, Workspace)> = captures
        .iter()
        .filter(|(c, _)| c == "fndef")
        .map(|(_, n)| (n.id(), (*n, Workspace::default())))
        .collect();
    debug!("Collecting function signatures.");
    let public_function = public_function(parsed_file);
    if let Some(pf) = &public_function {
        db_set_function(&sender, Arc::new(pf.clone()), thread.clone())?;
    }
    for node in functions
        .iter()
        .map(|(_, (node, _))| *node)
        .filter(|n| n.kind() == "function_definition")
        .collect::<Vec<Node>>()
    {
        let signature = function_signature(parsed_file, node)?;
        debug!("Got signature for {}", signature.name);
        let mut definition = FunctionDefinition {
            loc: signature.name_range,
            name: signature.name.clone(),
            path: parsed_file.path.clone(),
            signature: signature.clone(),
            package: String::new(),
        };
        if let Some(pf) = &public_function {
            if pf.loc == definition.loc {
                definition = pf.clone();
            }
        }
        let definition = Arc::new(definition);
        if let Some(parent) = parent_function(node) {
            if let Some((_, ws)) = functions.get_mut(&parent.id()) {
                debug!("Adding function {} to parent.", signature.name);
                ws.functions.insert(signature.name.clone(), definition);
            }
        } else {
            debug!("Adding function {} to base workspace.", signature.name);
            workspace
                .functions
                .insert(signature.name.clone(), definition);
        }
    }
    let mut scopes: Vec<usize> = vec![];
    for (capture, node) in captures {
        debug!("Current capture: {capture}.");
        if capture == "fndef" {
            continue;
        }
        scopes.clear();
        let mut p_node = *node;
        while let Some(parent) = parent_function(p_node) {
            scopes.push(parent.id());
            p_node = parent;
        }
        let name = node.utf8_text(parsed_file.contents.as_bytes())?.to_string();
        debug!("Got node {name}.");
        match capture.as_str() {
            "vardef" => def_var(
                name,
                &mut workspace,
                &scopes,
                &mut functions,
                *node,
                parsed_file,
            )?,
            "command" => command_capture_impl(
                name,
                &mut workspace,
                &scopes,
                &mut functions,
                sender.clone(),
                receiver.clone(),
                thread.clone(),
                node,
                parsed_file,
            )?,
            "fncall" => fncall_capture_impl(
                &mut workspace,
                &scopes,
                &mut functions,
                sender.clone(),
                receiver.clone(),
                thread.clone(),
                node,
                parsed_file,
            )?,
            "identifier" => {
                debug!("Defining identifier reference.");
                if let Some(parent) = node.parent() {
                    if parent.kind() == "field_expression"
                        || parent.kind() == "function_definition"
                        || parent.kind() == "multioutput_variable"
                    {
                        debug!("Node is part of something greater, leaving.");
                        continue;
                    }
                    if parent.kind() == "function_call" {
                        if let Some(parent) = parent.parent() {
                            if parent.kind() == "field_expression" {
                                // obj(1).bla = bla
                                // ^^^ cannot be a reference to Undefined
                                continue;
                            }
                        }
                    }
                    if node.utf8_text(parsed_file.contents.as_bytes())? == "end"
                        && (parent.kind() == "arguments" || parent.kind() == "range")
                    {
                        continue;
                    }
                }
                if let Some(parent) = parent_of_kind("assignment", *node) {
                    if let Some(left) = parent.child_by_field_name("left") {
                        if Range::from(left.range()).contains(node.start_position()) {
                            debug!("Identifier in assignment left:, skipping.");
                            continue;
                        }
                    }
                }
                if !workspace
                    .references
                    .iter()
                    .any(|r| r.borrow().loc == node.range().into())
                {
                    debug!("No references found at point.");
                    let mut vs = vec![];
                    for vref in ref_to_var(
                        name.clone(),
                        &mut workspace,
                        &scopes,
                        &mut functions,
                        *node,
                        parsed_file,
                    )? {
                        if let ReferenceTarget::Variable(v) = &vref.target {
                            if let Some(parent) = parent_of_kind("assignment", *node) {
                                if let Some(left) = parent.child_by_field_name("left") {
                                    if !Range::from(left.range()).fully_contains(v.borrow().loc) {
                                        vs.push(vref.clone());
                                    }
                                } else {
                                    vs.push(vref.clone());
                                }
                            } else {
                                vs.push(vref.clone());
                            }
                        }
                    }
                    if let Some(v) = vs.first() {
                        let vref = Arc::new(AtomicRefCell::new(v.clone()));
                        workspace.references.push(vref);
                    } else {
                        let vref = Reference {
                            loc: node.range().into(),
                            name,
                            target: ReferenceTarget::UnknownVariable,
                        };
                        let vref = Arc::new(AtomicRefCell::new(vref));
                        workspace.references.push(vref);
                    }
                } else {
                    debug!("Reference already exists.");
                }
            }
            "field" => field_capture_impl(
                &mut workspace,
                &scopes,
                &mut functions,
                sender.clone(),
                receiver.clone(),
                thread.clone(),
                node,
                parsed_file,
            )?,
            _ => {
                error!("Unknown capture {capture}");
            }
        }
    }
    for (_, ws) in functions.values_mut() {
        let ws = ws.clone();
        workspace.functions.extend(ws.functions);
        workspace.references.extend(ws.references);
        workspace.variables.extend(ws.variables);
    }
    Ok(workspace)
}

#[allow(clippy::too_many_arguments)]
fn command_capture_impl(
    name: String,
    workspace: &mut Workspace,
    scopes: &[usize],
    functions: &mut HashMap<usize, (Node, Workspace)>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    thread: SenderThread,
    node: &Node,
    parsed_file: &mut ParsedFile,
) -> Result<()> {
    debug!("Defining command [{name}].");
    match name.to_lowercase().as_str() {
        "load" => {
            debug!("It's a load.");
            if let Some(parent) = node.parent() {
                let mut cursor = parent.walk();
                for arg in parent
                    .named_children(&mut cursor)
                    .filter(|c| c.kind() == "command_argument")
                    .skip(1)
                {
                    let varname = arg.utf8_text(parsed_file.contents.as_bytes())?.to_string();
                    def_var(varname, workspace, scopes, functions, arg, parsed_file)?;
                }
            }
        }
        "import" => {
            debug!("It's an import.");
            if let Some(parent) = node.parent() {
                let mut cursor = parent.walk();
                for arg in parent
                    .named_children(&mut cursor)
                    .filter(|n| n.kind() == "command_argument")
                {
                    import_capture_impl(
                        workspace,
                        sender.clone(),
                        receiver.clone(),
                        thread.clone(),
                        &arg,
                        parsed_file,
                    )?;
                }
            }
        }
        "clear" | "clearvars" => {
            debug!("It's a clear.");
            if let Some(parent) = node.parent() {
                let mut cursor = parent.walk();
                let args: Vec<Node> = parent
                    .named_children(&mut cursor)
                    .filter(|n| n.kind() == "command_argument")
                    .collect();
                if args.is_empty() {
                    for (_, ws) in scopes.iter().flat_map(|s| functions.get(s)) {
                        for var in &ws.variables {
                            let mut var_mr = var.borrow_mut();
                            if var_mr.cleared == 0 && !var_mr.is_global {
                                var_mr.cleared = node.range().start_point.row;
                            }
                        }
                    }
                    if scopes.is_empty() {
                        for var in &workspace.variables {
                            let mut var_mr = var.borrow_mut();
                            if var_mr.cleared == 0 && !var_mr.is_global {
                                var_mr.cleared = node.range().start_point.row;
                            }
                        }
                    }
                }
                let mut delete = vec![];
                let mut keep = vec![];
                let mut except = false;
                let mut globals = false;
                for arg in args {
                    let text = arg.utf8_text(parsed_file.contents.as_bytes())?;
                    if text.to_lowercase() == "global" {
                        globals = true;
                        continue;
                    }
                    if text.to_lowercase() == "-except" && name.to_lowercase() == "clearvars" {
                        except = true;
                        continue;
                    }
                    if text.starts_with('-') {
                        debug!("It's an option argument, we dont that here.");
                        break;
                    }
                    if !except {
                        delete.push(text);
                    } else {
                        keep.push(text);
                    }
                }
                if delete.is_empty() {
                    delete.push("*");
                }
                let mut ws: Vec<&Workspace> = scopes
                    .iter()
                    .flat_map(|s| functions.get(s))
                    .map(|(_, ws)| ws)
                    .collect();
                if ws.is_empty() {
                    ws.push(workspace);
                }
                for ws in ws {
                    'var: for var in &ws.variables {
                        let mut var_mr = var.borrow_mut();
                        for text in &delete {
                            let text = text.replace('*', ".*");
                            let text = format!("^{}$", text);
                            if let Ok(sw) = Regex::new(text.as_str()) {
                                if sw.is_match(var_mr.name.as_str()) {
                                    for text in &keep {
                                        let text = text.replace('*', ".*");
                                        let text = format!("^{}$", text);
                                        if let Ok(sw) = Regex::new(text.as_str()) {
                                            if sw.is_match(var_mr.name.as_str()) {
                                                continue 'var;
                                            }
                                        }
                                    }
                                    if var_mr.cleared == 0 && (!var_mr.is_global || globals) {
                                        var_mr.cleared = node.range().start_point.row;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        "syms" => {
            debug!("It's a syms.");
            if let Some(parent) = node.parent() {
                let mut cursor = parent.walk();
                let children: Vec<Node> = parent
                    .named_children(&mut cursor)
                    .filter(|n| n.kind() == "command_argument")
                    .collect();
                let regex = Regex::new(r"^[a-zA-Z_][a-zA-Z_0-9]*$")?;
                for (i, arg) in children.iter().enumerate() {
                    let text = arg.utf8_text(parsed_file.contents.as_bytes())?;
                    if i == children.len() - 1
                        && (text == "matrix"
                            || text == "clear"
                            || text == "real"
                            || text == "positive")
                    {
                        break;
                    } else if regex.is_match(text) {
                        def_var(
                            text.to_owned(),
                            workspace,
                            scopes,
                            functions,
                            *arg,
                            parsed_file,
                        )?;
                    } else {
                        break;
                    }
                }
            }
        }
        _ => {
            debug!("It's unknown ({name}).");
            // Commands are searched for in the path.
            if let Some(ms) = db_get_script(&sender, &receiver, name.clone(), thread.clone()) {
                let r = Reference {
                    loc: node.range().into(),
                    name: name.clone(),
                    target: ReferenceTarget::Script(ms.path.clone()),
                };
                let r = Arc::new(AtomicRefCell::new(r));
                workspace.references.push(r);
            } else {
                debug!("Not a script.");
                let fs = ref_to_fn(
                    name,
                    workspace,
                    scopes,
                    functions,
                    sender.clone(),
                    receiver.clone(),
                    thread,
                    *node,
                    false,
                )?;
                if let Some(fref) = fs.first() {
                    let fref = Arc::new(AtomicRefCell::new(fref.clone()));
                    workspace.references.push(fref);
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn fncall_capture_impl(
    workspace: &mut Workspace,
    scopes: &[usize],
    functions: &mut HashMap<usize, (Node, Workspace)>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    thread: SenderThread,
    node: &Node,
    parsed_file: &mut ParsedFile,
) -> Result<()> {
    if let Some(parent) = node.parent() {
        if parent.kind() == "field_expression" {
            return Ok(());
        }
    }
    debug!("Analysing function call.");
    if let Some(name_node) = node.child_by_field_name("name") {
        if name_node.kind() == "identifier"
            && !workspace
                .references
                .iter()
                .map(|f| f.borrow().loc)
                .any(|loc| loc == name_node.range().into())
        {
            if let Ok(fname) = name_node
                .utf8_text(parsed_file.contents.as_bytes())
                .map(String::from)
            {
                debug!("Defining function call {fname}.");
                let vs = ref_to_var(
                    fname.clone(),
                    workspace,
                    scopes,
                    functions,
                    name_node,
                    parsed_file,
                )?;
                if let Some(v) = vs.first() {
                    let v = Arc::new(AtomicRefCell::new(v.clone()));
                    workspace.references.push(v);
                    return Ok(());
                }
                let fs = ref_to_fn(
                    fname.clone(),
                    workspace,
                    scopes,
                    functions,
                    sender.clone(),
                    receiver.clone(),
                    thread,
                    name_node,
                    false,
                )?;
                if let Some(fref) = fs.first() {
                    let fref = Arc::new(AtomicRefCell::new(fref.clone()));
                    workspace.references.push(fref);
                } else {
                    let right_def = if let Some(parent) = parent_of_kind("assignment", *node) {
                        if let Some(right) = parent.child_by_field_name("right") {
                            Range::from(right.range()).contains(node.start_position())
                        } else {
                            true
                        }
                    } else {
                        true
                    };
                    if right_def {
                        let r = Reference {
                            loc: name_node.range().into(),
                            name: fname.clone(),
                            target: ReferenceTarget::UnknownFunction,
                        };
                        let fref = Arc::new(AtomicRefCell::new(r));
                        workspace.references.push(fref);
                    }
                }
            }
        }
    }
    Ok(())
}

fn import_capture_impl(
    workspace: &mut Workspace,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    thread: SenderThread,
    node: &Node,
    parsed_file: &mut ParsedFile,
) -> Result<()> {
    if let Ok(path) = node.utf8_text(parsed_file.contents.as_bytes()) {
        debug!("Importing {path}");
        let functions = db_fetch_functions(&sender, &receiver, thread).unwrap_or(HashMap::new());
        if let Some(path) = path.strip_suffix(".*") {
            debug!("Importing all functions from {path}");
            for (f_name, f_def) in &functions {
                let (package, name) = pkg_basename(f_name.clone());
                if package == path {
                    debug!("Importing {f_name} as {name}");
                    workspace.functions.insert(name, Arc::clone(f_def));
                }
            }
        } else {
            debug!("Importing single function.");
            if let Some(f_def) = functions.get(path) {
                let (_, name) = pkg_basename(path.into());
                debug!("Importing {path} as {name}");
                workspace.functions.insert(name, Arc::clone(f_def));
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn field_capture_impl(
    workspace: &mut Workspace,
    scopes: &[usize],
    functions: &mut HashMap<usize, (Node, Workspace)>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    thread: SenderThread,
    node: &Node,
    parsed_file: &mut ParsedFile,
) -> Result<()> {
    debug!("Defining field expression.");
    let is_def = if let Some(parent) = node.parent() {
        if parent.kind() == "multioutput_variable" {
            true
        } else if parent.kind() == "assignment" {
            if let Some(left) = parent.child_by_field_name("left") {
                left.id() == node.id()
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };
    debug!("Is definition: {is_def}");
    if let Some(object) = node.child_by_field_name("object") {
        if object.kind() == "function_call" {
            if let Some(name_node) = object.child_by_field_name("name") {
                if "identifier" == name_node.kind() {
                    if let Ok(name) = name_node
                        .utf8_text(parsed_file.contents.as_bytes())
                        .map(String::from)
                    {
                        let vs = ref_to_var(
                            name.clone(),
                            workspace,
                            scopes,
                            functions,
                            name_node,
                            parsed_file,
                        )?;
                        let fs = ref_to_fn(
                            name.clone(),
                            workspace,
                            scopes,
                            functions,
                            sender.clone(),
                            receiver.clone(),
                            thread,
                            name_node,
                            false,
                        )?;
                        if let Some(v) = vs.iter().chain(fs.iter()).next() {
                            let r = Arc::new(AtomicRefCell::new(v.clone()));
                            workspace.references.push(r);
                        }
                        if is_def {
                            def_var(name, workspace, scopes, functions, name_node, parsed_file)?;
                        }
                    }
                }
            }
            return Ok(());
        }
        let base_name = object
            .utf8_text(parsed_file.contents.as_bytes())?
            .to_string();
        let mut cursor = node.walk();
        let mut fields = vec![];
        for field in node.children_by_field_name("field", &mut cursor) {
            // Avoids false completions where the current line merges with the next to make a field_expression
            // It's probably wrong anyway...
            if field.range().start_point.row != node.range().start_point.row {
                break;
            }
            match field.kind() {
                "identifier" => {
                    if let Ok(name) = field
                        .utf8_text(parsed_file.contents.as_bytes())
                        .map(String::from)
                    {
                        fields.push((name, field));
                    } else {
                        break;
                    }
                }
                "function_call" => {
                    if let Some(name) = field.child_by_field_name("name") {
                        if let Ok(fname) = name
                            .utf8_text(parsed_file.contents.as_bytes())
                            .map(String::from)
                        {
                            fields.push((fname, name));
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        let bo = vec![(base_name, object)];
        let fields: Vec<(String, Node)> =
            bo.iter().chain(fields.iter()).map(Clone::clone).collect();
        let mut is_pack = false;
        let mut current_ns: Option<String> = None;
        for (i, (name, field)) in fields.iter().enumerate() {
            let path = fields.iter().take(i + 1).map(|(n, _)| n).join(".");
            if is_def {
                // Definitions can shadow namespaces, so we don't care about namespaces here.
                let vref = ref_to_var(
                    path.clone(),
                    workspace,
                    scopes,
                    functions,
                    *field,
                    parsed_file,
                )?;
                if let Some(v) = vref.first() {
                    let r = Arc::new(AtomicRefCell::new(v.clone()));
                    workspace.references.push(r);
                    continue;
                } else if i > 0 {
                    let reference = Reference {
                        loc: field.range().into(),
                        name: path.clone(),
                        target: ReferenceTarget::UnknownVariable,
                    };
                    let reference = Arc::new(AtomicRefCell::new(reference));
                    workspace.references.push(reference);
                }
                if i == 0 || i == fields.len().saturating_sub(1) {
                    def_var(path, workspace, scopes, functions, *field, parsed_file)?;
                }
            } else {
                // If it is not a definition, it can be a namespace
                // But we first need to make sure it is not a variable.
                if i == 0 {
                    let vref = ref_to_var(
                        path.clone(),
                        workspace,
                        scopes,
                        functions,
                        *field,
                        parsed_file,
                    )?;
                    is_pack = vref.first().is_none();
                }
                if is_pack {
                    debug!("It's a package.");
                    // The base name is a package, so only packages, functions, classes and class
                    // folders are allowed here.
                    if let Some(ns) = current_ns.take() {
                        debug!("Is [{name}] a subpackage, function, or class?");
                        let pkg = format!("{ns}.{name}");
                        let pkg = pkg.strip_prefix('.').map(String::from).unwrap_or(pkg);
                        let ws = db_get_package(&sender, &receiver, pkg, thread.clone());
                        let ws = ws.iter().min_by(|a, b| a.len().cmp(&b.len()));
                        if let Some(parent) = field.parent() {
                            if parent.kind() == "function_call" {
                                debug!("Looking for function {path}");
                                // This is a function call, so look for functions.
                                if let Some(f_def) = db_get_function(
                                    &sender,
                                    &receiver,
                                    path.clone(),
                                    thread.clone(),
                                ) {
                                    debug!("Got function for {path}.");
                                    let f_def =
                                        Arc::new(AtomicRefCell::new(f_def.as_ref().clone()));
                                    let vref = Reference {
                                        loc: field.range().into(),
                                        name: path,
                                        target: ReferenceTarget::Function(f_def),
                                    };
                                    let vref = Arc::new(AtomicRefCell::new(vref));
                                    workspace.references.push(vref);
                                } else {
                                    debug!("Unknown function for path {path}");
                                    let vref = Reference {
                                        loc: field.range().into(),
                                        name: path,
                                        target: ReferenceTarget::UnknownFunction,
                                    };
                                    let vref = Arc::new(AtomicRefCell::new(vref));
                                    workspace.references.push(vref);
                                    return Ok(());
                                }
                            } else {
                                // This is not a function call, so look for namespaces and
                                // classfolders.
                                debug!("Not a function/class. Subpackage?");
                                if let Some(ws) = ws {
                                    debug!("Yep, subpackage.");
                                    let vref = Reference {
                                        loc: field.range().into(),
                                        name: path,
                                        target: ReferenceTarget::Namespace(ws.clone()),
                                    };
                                    let vref = Arc::new(AtomicRefCell::new(vref));
                                    workspace.references.push(vref);
                                    current_ns = Some(ws.clone());
                                } else {
                                    debug!("Something undefined.");
                                    let vref = Reference {
                                        loc: field.range().into(),
                                        name: path,
                                        target: ReferenceTarget::UnknownVariable,
                                    };
                                    let vref = Arc::new(AtomicRefCell::new(vref));
                                    workspace.references.push(vref);
                                    return Ok(());
                                }
                            }
                        } else {
                            return Err(code_loc!("Node has no parent."));
                        }
                    } else if let Some(ns) =
                        db_get_package(&sender, &receiver, name.clone(), thread.clone())
                            .iter()
                            .min_by(|a, b| a.len().cmp(&b.len()))
                    {
                        debug!("First package found: {name}");
                        let vref = Reference {
                            loc: field.range().into(),
                            name: path,
                            target: ReferenceTarget::Namespace(ns.clone()),
                        };
                        let vref = Arc::new(AtomicRefCell::new(vref));
                        workspace.references.push(vref);
                        current_ns = Some(ns.clone());
                    } else {
                        let r = db_get_package(&sender, &receiver, name.clone(), thread.clone());
                        debug!("Don't know what it is. Leaving. {name}:{r:?}");
                        return Ok(());
                    }
                } else {
                    debug!("It's a variable, not a package.");
                    // The base name is a variable, so act normal
                    let vs = ref_to_var(
                        path.clone(),
                        workspace,
                        scopes,
                        functions,
                        *field,
                        parsed_file,
                    )?;
                    if let Some(v) = vs.first() {
                        let v = Arc::new(AtomicRefCell::new(v.clone()));
                        workspace.references.push(v);
                    } else {
                        debug!("Could not find definition for {path}.");
                        let vref = Reference {
                            loc: field.range().into(),
                            name: path.clone(),
                            target: ReferenceTarget::UnknownVariable,
                        };
                        let vref = Arc::new(AtomicRefCell::new(vref));
                        workspace.references.push(vref);
                    }
                }
            }
        }
    }
    Ok(())
}

fn ref_to_var(
    name: String,
    workspace: &mut Workspace,
    scopes: &[usize],
    functions: &mut HashMap<usize, (Node, Workspace)>,
    node: Node,
    parsed_file: &mut ParsedFile,
) -> Result<Vec<Reference>> {
    let mut references = vec![];
    let (is_assignment, p_range) = if let Some(parent) = parent_of_kind("assignment", node) {
        if let Some(left) = parent.child_by_field_name("left") {
            (true, left.range().into())
        } else {
            (false, Range::default())
        }
    } else {
        (false, Range::default())
    };
    for (_, ws) in scopes.iter().flat_map(|i| functions.get(i)) {
        for v in ws.variables.iter().rev() {
            let v_ref = v.borrow();
            if v_ref.cleared > 0 {
                continue;
            }
            if v_ref.name == name {
                if is_assignment && p_range.fully_contains(v_ref.loc) {
                    continue;
                }
                if let Some(ndef) = node_at_pos(parsed_file, v_ref.loc.start) {
                    if !is_in_soft_scope(node, ndef) {
                        continue;
                    }
                }
                let r = Reference {
                    loc: node.range().into(),
                    name: name.clone(),
                    target: ReferenceTarget::Variable(v.clone()),
                };
                references.push(r);
            }
        }
    }
    // If scope is not empty, we cannot look at the global workspace as this is a private function
    // of a script, and therefore scoped. If this is a nested function, everything it can see was
    // covered alread in the previous for loop.
    // Except for lambdas.
    if scopes.is_empty()
        || scopes
            .iter()
            .flat_map(|s| functions.get(s))
            .all(|(n, _)| n.kind() == "lambda")
    {
        for v in workspace.variables.iter().rev() {
            let v_ref = v.borrow();
            if v_ref.cleared > 0 {
                continue;
            }
            if v_ref.name == name {
                if is_assignment && p_range.fully_contains(v_ref.loc) {
                    continue;
                }
                if let Some(ndef) = node_at_pos(parsed_file, v_ref.loc.start) {
                    if !is_in_soft_scope(node, ndef) {
                        continue;
                    }
                }
                let r = Reference {
                    loc: node.range().into(),
                    name: name.clone(),
                    target: ReferenceTarget::Variable(v.clone()),
                };
                references.push(r);
            }
        }
    }
    Ok(references)
}

fn ref_to_fn_in_ws(
    name: String,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    thread: SenderThread,
    node: Node,
    pkg: bool,
) -> Result<Vec<Reference>> {
    let mut references = vec![];
    for fn_def in db_fetch_functions(&sender, &receiver, thread)
        .unwrap_or(HashMap::new())
        .values()
    {
        if fn_def.name == name && (fn_def.package.is_empty() || pkg) {
            let f_ref = Reference {
                loc: node.range().into(),
                name: name.clone(),
                target: ReferenceTarget::Function(Arc::new(AtomicRefCell::new(
                    fn_def.as_ref().clone(),
                ))),
            };
            references.push(f_ref);
        }
    }
    Ok(references)
}

#[allow(clippy::too_many_arguments)]
fn ref_to_fn(
    name: String,
    workspace: &mut Workspace,
    scopes: &[usize],
    functions: &mut HashMap<usize, (Node, Workspace)>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    thread: SenderThread,
    node: Node,
    pkg: bool,
) -> Result<Vec<Reference>> {
    let mut references = vec![];
    for (_, ws) in scopes.iter().flat_map(|i| functions.get(i)) {
        for f in ws.functions.values() {
            if f.name == name {
                let r = Reference {
                    loc: node.range().into(),
                    name: name.clone(),
                    target: ReferenceTarget::Function(Arc::new(AtomicRefCell::new(
                        f.as_ref().clone(),
                    ))),
                };
                references.push(r);
            }
        }
    }
    for f in workspace.functions.values() {
        if f.name == name {
            let r = Reference {
                loc: node.range().into(),
                name: name.clone(),
                target: ReferenceTarget::Function(Arc::new(AtomicRefCell::new(f.as_ref().clone()))),
            };
            references.push(r);
        }
    }
    let fs = ref_to_fn_in_ws(name, sender.clone(), receiver.clone(), thread, node, pkg)?;
    references.extend(fs);
    Ok(references)
}

fn def_var(
    name: String,
    workspace: &mut Workspace,
    scopes: &[usize],
    functions: &mut HashMap<usize, (Node, Workspace)>,
    node: Node,
    parsed_file: &mut ParsedFile,
) -> Result<()> {
    debug!("Defining variable {name}");
    let mut cursor = node.walk();
    // If it is a variable definition inside a function which has an output argument of same name,
    // point to that instead of creating a new definition.
    if let Some(parent) = parent_function(node) {
        if let Some(output) = parent
            .named_children(&mut cursor)
            .find(|n| n.kind() == "function_output")
        {
            let mut ps = vec![];
            if let Some(output) = output.named_child(0) {
                if output.kind() == "identifier" {
                    ps.push(output.start_position());
                } else if output.kind() == "multioutput_variable" {
                    let mut cursor = output.walk();
                    for output in output.named_children(&mut cursor) {
                        ps.push(output.start_position());
                    }
                }
            }
            for p in ps {
                if let Some(scope) = scopes.first() {
                    if let Some((_, ws)) = functions.get(scope) {
                        for var in &ws.variables {
                            let v_ref = var.borrow();
                            if v_ref.name == name && v_ref.loc.contains(p) {
                                let reference = Reference {
                                    loc: node.range().into(),
                                    name,
                                    target: ReferenceTarget::Variable(var.clone()),
                                };
                                let referece = Arc::new(AtomicRefCell::new(reference));
                                workspace.references.push(referece);
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }
    }
    let vref = ref_to_var(
        name.clone(),
        workspace,
        scopes,
        functions,
        node,
        parsed_file,
    )?;
    if parent_of_kind("function_call", node).is_some() && !vref.is_empty() {
        return Ok(());
    }
    if soft_scope_parent(node).is_some() && !vref.is_empty() {
        let vref = Arc::new(AtomicRefCell::new(vref.first().unwrap().clone()));
        workspace.references.push(vref);
    } else {
        let is_global = parent_of_kind("global_operator", node).is_some();
        let is_parameter = parent_of_kind("function_output", node).is_some()
            || parent_of_kind("function_arguments", node).is_some();
        let definition = VariableDefinition {
            loc: node.range().into(),
            name: name.clone(),
            cleared: 0,
            is_parameter,
            is_global,
        };
        let definition = Arc::new(AtomicRefCell::new(definition));
        if let Some(scope) = scopes.first() {
            if let Some((_, ws)) = functions.get_mut(scope) {
                ws.variables.push(definition);
            }
        } else {
            workspace.variables.push(definition);
        }
    }
    Ok(())
}

/// Verifies if some and other are in the same soft-scope. A soft-scope is introduced by any
/// statement with multiple blocks. This definition is necessary to avoid variables in a branch of
/// an if/elseif/else or case/otherwise or try/catch to reference each other instead of the
/// variable before the block.
fn is_in_soft_scope(nref: Node, ndef: Node) -> bool {
    let mut node = nref;
    loop {
        if let Some(parent) = soft_scope_parent(node) {
            let range: Range = parent.range().into();
            if range.contains(nref.start_position()) && range.contains(ndef.start_position()) {
                let mut cursor = parent.walk();
                for child in parent.named_children(&mut cursor) {
                    let range: Range = child.range().into();
                    if range.contains(ndef.start_position())
                        && !range.contains(nref.start_position())
                    {
                        return false;
                    }
                }
            }
            node = parent;
        } else {
            return true;
        }
    }
}

fn soft_scope_parent(node: Node) -> Option<Node> {
    let mut node = node;
    loop {
        if let Some(parent) = node.parent() {
            if parent.kind() == "if_statement"
                || parent.kind() == "switch_statement"
                || parent.kind() == "try_statement"
                || parent.kind() == "for_statement"
                || parent.kind() == "while_statement"
            {
                return Some(parent);
            }
            node = parent;
        } else {
            return None;
        }
    }
}

fn parent_function(node: Node) -> Option<Node> {
    let mut node = node;
    loop {
        if let Some(parent) = node.parent() {
            if parent.kind() == "function_definition" || parent.kind() == "lambda" {
                return Some(parent);
            }
            node = parent;
        } else {
            return None;
        }
    }
}

pub fn parent_of_kind<S: Into<String>>(kind: S, node: Node) -> Option<Node> {
    let kind: String = kind.into();
    let mut node = node;
    loop {
        if let Some(parent) = node.parent() {
            if parent.kind() == kind {
                return Some(parent);
            }
            node = parent;
        } else {
            return None;
        }
    }
}

fn node_at_pos(parsed_file: &mut ParsedFile, point: Point) -> Option<Node> {
    parsed_file
        .tree
        .root_node()
        .named_descendant_for_point_range(point, point)
}

fn pkg_basename(s: String) -> (String, String) {
    let parts: Vec<String> = s.rsplitn(2, '.').map(String::from).collect();
    if parts.len() != 2 {
        ("".into(), s)
    } else {
        (parts[1].clone(), parts[0].clone())
    }
}
