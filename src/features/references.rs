/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use atomic_refcell::AtomicRefCell;
use crossbeam_channel::{Receiver, Sender};
use log::debug;
use lsp_types::{DocumentHighlightKind, Location, Url};
use tree_sitter::Point;

use crate::code_loc;
use crate::extractors::symbols::parent_of_kind;
use crate::threads::db::{db_fetch_parsed_files, db_get_parsed_file};
use crate::types::{
    FunctionDefinition, ParsedFile, ReferenceTarget, SenderThread, ThreadMessage,
    VariableDefinition,
};

pub fn find_references_to_symbol(
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    path: String,
    loc: Point,
    inc_dec: bool,
) -> Result<Vec<(Location, DocumentHighlightKind)>> {
    debug!("Listing references.");
    let file = db_get_parsed_file(&sender, &receiver, path, SenderThread::Handler)
        .ok_or(code_loc!("No such file."))?;
    for r in &file.workspace.references {
        let r_ref = r.borrow();
        if r_ref.loc.contains(loc) {
            let target = r_ref.target.clone();
            match &target {
                ReferenceTarget::Function(f) => {
                    drop(r_ref);
                    drop(file);
                    return find_references_to_function(
                        sender.clone(),
                        receiver.clone(),
                        f.clone(),
                        inc_dec,
                    );
                }
                ReferenceTarget::Script(f) => {
                    drop(r_ref);
                    drop(file);
                    return find_references_to_script(
                        sender.clone(),
                        receiver.clone(),
                        f.to_owned(),
                    );
                }
                ReferenceTarget::Variable(v) => {
                    drop(r_ref);
                    return find_references_to_variable(&file, v.clone(), inc_dec);
                }
                ReferenceTarget::UnknownVariable => {
                    let name = r_ref.name.clone();
                    drop(r_ref);
                    if name.contains('.') {
                        return find_references_to_field(&file, name, loc);
                    }
                    return Ok(vec![]);
                }
                ReferenceTarget::Namespace(ns) => {
                    drop(r_ref);
                    return find_references_to_namespace(&file, ns.clone());
                }
                _ => return Ok(vec![]),
            }
        }
    }
    for v in &file.workspace.variables {
        if v.borrow().loc.contains(loc) {
            return find_references_to_variable(&file, v.clone(), inc_dec);
        }
    }
    for f in file.workspace.functions.values() {
        if f.loc.contains(loc) {
            let function = Arc::clone(f);
            return find_references_to_function(
                sender.clone(),
                receiver.clone(),
                Arc::new(AtomicRefCell::new(function.as_ref().clone())),
                inc_dec,
            );
        }
    }
    Ok(vec![])
}

fn find_references_to_function(
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    function: Arc<AtomicRefCell<FunctionDefinition>>,
    inc_dec: bool,
) -> Result<Vec<(Location, DocumentHighlightKind)>> {
    let mut refs = vec![];
    for (path, file) in
        db_fetch_parsed_files(&sender, &receiver, SenderThread::Handler).unwrap_or(HashMap::new())
    {
        let f_refs = file.workspace.references.iter().map(|r| (path.clone(), r));
        for (r_path, reference) in f_refs {
            let r_ref = reference.borrow();
            if let ReferenceTarget::Function(target) = &r_ref.target {
                if function.borrow().path == target.borrow().path {
                    let path = String::from("file://") + r_path.as_str();
                    let uri = Url::parse(path.as_str())?;
                    let location = Location::new(uri.clone(), r_ref.loc.into());
                    refs.push((location, DocumentHighlightKind::TEXT));
                }
            }
        }
    }
    if inc_dec {
        let v_ref = function.borrow();
        let path = v_ref.path.clone();
        let path = String::from("file://") + path.as_str();
        let uri = Url::parse(path.as_str())?;
        let loc = v_ref.signature.name_range;
        let location = Location::new(uri.clone(), loc.into());
        refs.push((location, DocumentHighlightKind::TEXT));
    }
    Ok(refs)
}

fn find_references_to_script(
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    script: String,
) -> Result<Vec<(Location, DocumentHighlightKind)>> {
    let mut refs = vec![];
    for (path, file) in
        db_fetch_parsed_files(&sender, &receiver, SenderThread::Handler).unwrap_or(HashMap::new())
    {
        let f_refs = file.workspace.references.iter().map(|r| (path.clone(), r));
        for (r_path, reference) in f_refs {
            let r_ref = reference.borrow();
            if let ReferenceTarget::Script(target) = &r_ref.target {
                if script == *target {
                    let path = String::from("file://") + r_path.as_str();
                    let uri = Url::parse(path.as_str())?;
                    let location = Location::new(uri.clone(), r_ref.loc.into());
                    refs.push((location, DocumentHighlightKind::TEXT));
                }
            }
        }
    }
    Ok(refs)
}

fn find_references_to_variable(
    parsed_file: &ParsedFile,
    variable: Arc<AtomicRefCell<VariableDefinition>>,
    inc_dec: bool,
) -> Result<Vec<(Location, DocumentHighlightKind)>> {
    let path = String::from("file://") + parsed_file.path.as_str();
    let uri = Url::parse(path.as_str())?;
    let mut refs = vec![];
    for r in &parsed_file.workspace.references {
        let r_ref = r.borrow();
        if let ReferenceTarget::Variable(v) = &r_ref.target {
            if variable.borrow().loc == v.borrow().loc {
                let location = Location::new(uri.clone(), r_ref.loc.into());
                refs.push((location, DocumentHighlightKind::READ));
            }
        }
    }
    if inc_dec {
        let loc = variable.borrow().loc;
        let location = Location::new(uri.clone(), loc.into());
        refs.push((location, DocumentHighlightKind::WRITE));
    }
    Ok(refs)
}

fn find_references_to_namespace(
    parsed_file: &ParsedFile,
    ns: String,
) -> Result<Vec<(Location, DocumentHighlightKind)>> {
    let path = String::from("file://") + parsed_file.path.as_str();
    let uri = Url::parse(path.as_str())?;
    let mut refs = vec![];
    for r in &parsed_file.workspace.references {
        let r_ref = r.borrow();
        if let ReferenceTarget::Namespace(v) = &r_ref.target {
            if ns == *v {
                let location = Location::new(uri.clone(), r_ref.loc.into());
                refs.push((location, DocumentHighlightKind::TEXT));
            }
        }
    }
    Ok(refs)
}

fn find_references_to_field(
    parsed_file: &ParsedFile,
    name: String,
    pos: Point,
) -> Result<Vec<(Location, DocumentHighlightKind)>> {
    let path = String::from("file://") + parsed_file.path.as_str();
    let uri = Url::parse(path.as_str())?;
    let mut rs = vec![];
    if let Some(base_def) = base_definition(parsed_file, pos) {
        for r in &parsed_file.workspace.references {
            let r_ref = r.borrow();
            if r_ref.name == name {
                let range = r_ref.loc;
                let pos = r_ref.loc.start;
                drop(r_ref);
                if let Some(def) = base_definition(parsed_file, pos) {
                    if base_def.borrow().loc == def.borrow().loc {
                        let location = Location::new(uri.clone(), range.into());
                        rs.push((location, DocumentHighlightKind::WRITE));
                    }
                }
            }
        }
    }
    Ok(rs)
}

fn base_definition(
    parsed_file: &ParsedFile,
    pos: Point,
) -> Option<Arc<AtomicRefCell<VariableDefinition>>> {
    let tree = parsed_file.tree.clone();
    let root = tree.root_node();
    if let Some(node) = root.descendant_for_point_range(pos, pos) {
        if let Some(parent) = parent_of_kind("field_expression", node) {
            let pos = parent.start_position();
            for r in &parsed_file.workspace.references {
                let r_ref = r.borrow();
                if r_ref.loc.contains(pos) {
                    if let ReferenceTarget::Variable(v) = &r_ref.target {
                        return Some(v.clone());
                    }
                }
            }
            for d in &parsed_file.workspace.variables {
                if d.borrow().loc.contains(pos) {
                    return Some(d.clone());
                }
            }
        }
    }
    None
}
