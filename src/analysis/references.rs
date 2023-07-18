/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::sync::{Arc, MutexGuard};

use anyhow::{anyhow, Result};
use atomic_refcell::{AtomicRefCell, AtomicRefMut};
use log::debug;
use lsp_types::{DocumentHighlightKind, Location, Url};
use tree_sitter::Point;

use crate::code_loc;
use crate::parsed_file::ParsedFile;
use crate::session_state::{Namespace, SessionState};
use crate::types::{ClassDefinition, FunctionDefinition, ReferenceTarget, VariableDefinition};

pub fn find_references_to_symbol<'a>(
    state: &'a MutexGuard<'a, &mut SessionState>,
    path: String,
    loc: Point,
    inc_dec: bool,
) -> Result<Vec<(Location, DocumentHighlightKind)>> {
    debug!("Listing references.");
    let file = state.files.get(&path).ok_or(code_loc!("No such file."))?;
    let pf_mr = file.borrow_mut();
    for r in &pf_mr.workspace.references {
        let r_ref = r.borrow();
        if r_ref.loc.contains(loc) {
            let target = r_ref.target.clone();
            match &target {
                ReferenceTarget::Class(c) => {
                    drop(r_ref);
                    drop(pf_mr);
                    return find_references_to_class(state, Arc::clone(c), inc_dec);
                }
                ReferenceTarget::Function(f) => {
                    drop(r_ref);
                    drop(pf_mr);
                    return find_references_to_function(state, Arc::clone(f), inc_dec);
                }
                ReferenceTarget::Script(f) => {
                    drop(r_ref);
                    drop(pf_mr);
                    return find_references_to_script(state, Arc::clone(f));
                }
                ReferenceTarget::Variable(v) => {
                    drop(r_ref);
                    return find_references_to_variable(&pf_mr, Arc::clone(v), inc_dec);
                }
                ReferenceTarget::UnknownVariable => {
                    let name = r_ref.name.clone();
                    drop(r_ref);
                    if name.contains('.') {
                        return find_references_to_field(&pf_mr, name, loc);
                    }
                    return Ok(vec![]);
                }
                ReferenceTarget::Namespace(ns) => {
                    drop(r_ref);
                    return find_references_to_namespace(&pf_mr, Arc::clone(ns));
                }
                _ => return Ok(vec![]),
            }
        }
    }
    for v in &pf_mr.workspace.variables {
        if v.borrow().loc.contains(loc) {
            return find_references_to_variable(&pf_mr, Arc::clone(v), inc_dec);
        }
    }
    for f in pf_mr.workspace.functions.values() {
        if f.borrow().loc.contains(loc) {
            let function = Arc::clone(f);
            drop(pf_mr);
            return find_references_to_function(state, function, inc_dec);
        }
    }
    for c in pf_mr.workspace.classes.values() {
        if c.borrow().loc.contains(loc) {
            let class = Arc::clone(c);
            drop(pf_mr);
            return find_references_to_class(state, class, inc_dec);
        }
    }
    Ok(vec![])
}

fn find_references_to_class<'a>(
    state: &'a MutexGuard<'a, &mut SessionState>,
    class: Arc<AtomicRefCell<ClassDefinition>>,
    inc_dec: bool,
) -> Result<Vec<(Location, DocumentHighlightKind)>> {
    let mut refs = vec![];
    for (path, file) in &state.files {
        let pf_ref = file.borrow();
        let f_refs = pf_ref
            .workspace
            .references
            .iter()
            .map(|r| (path.clone(), r));
        for (r_path, reference) in f_refs {
            let r_lock = reference.borrow();
            if let ReferenceTarget::Class(target) = &r_lock.target {
                if Arc::ptr_eq(&class, target) {
                    let path = String::from("file://") + r_path.as_str();
                    let uri = Url::parse(path.as_str())?;
                    let location = Location::new(uri.clone(), r_lock.loc.into());
                    refs.push((location, DocumentHighlightKind::TEXT));
                }
            }
        }
    }
    if inc_dec {
        let class_ref = class.borrow();
        let path = class_ref.parsed_file.borrow().path.clone();
        let path = String::from("file://") + path.as_str();
        let uri = Url::parse(path.as_str())?;
        let loc = class_ref.loc;
        let location = Location::new(uri.clone(), loc.into());
        refs.push((location, DocumentHighlightKind::TEXT));
    }
    Ok(refs)
}

fn find_references_to_function<'a>(
    state: &'a MutexGuard<'a, &mut SessionState>,
    function: Arc<AtomicRefCell<FunctionDefinition>>,
    inc_dec: bool,
) -> Result<Vec<(Location, DocumentHighlightKind)>> {
    let mut refs = vec![];
    for (path, file) in &state.files {
        let pf_ref = file.borrow();
        let f_refs = pf_ref
            .workspace
            .references
            .iter()
            .map(|r| (path.clone(), r));
        for (r_path, reference) in f_refs {
            let r_ref = reference.borrow();
            if let ReferenceTarget::Function(target) = &r_ref.target {
                if Arc::ptr_eq(&function, target) {
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
        let path = v_ref.parsed_file.borrow().path.clone();
        let path = String::from("file://") + path.as_str();
        let uri = Url::parse(path.as_str())?;
        let loc = v_ref.loc;
        let location = Location::new(uri.clone(), loc.into());
        refs.push((location, DocumentHighlightKind::TEXT));
    }
    Ok(refs)
}

fn find_references_to_script<'a>(
    state: &'a MutexGuard<'a, &mut SessionState>,
    script: Arc<AtomicRefCell<ParsedFile>>,
) -> Result<Vec<(Location, DocumentHighlightKind)>> {
    let mut refs = vec![];
    for (path, file) in &state.files {
        let pf_ref = file.borrow();
        let f_refs = pf_ref
            .workspace
            .references
            .iter()
            .map(|r| (path.clone(), r));
        for (r_path, reference) in f_refs {
            let r_ref = reference.borrow();
            if let ReferenceTarget::Script(target) = &r_ref.target {
                if Arc::ptr_eq(&script, target) {
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

fn find_references_to_variable<'a>(
    parsed_file: &AtomicRefMut<'_, ParsedFile>,
    variable: Arc<AtomicRefCell<VariableDefinition>>,
    inc_dec: bool,
) -> Result<Vec<(Location, DocumentHighlightKind)>> {
    let path = String::from("file://") + parsed_file.path.as_str();
    let uri = Url::parse(path.as_str())?;
    let mut refs = vec![];
    for r in &parsed_file.workspace.references {
        let r_ref = r.borrow();
        if let ReferenceTarget::Variable(v) = &r_ref.target {
            if Arc::ptr_eq(&variable, v) {
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

fn find_references_to_namespace<'a>(
    parsed_file: &AtomicRefMut<'_, ParsedFile>,
    ns: Arc<AtomicRefCell<Namespace>>,
) -> Result<Vec<(Location, DocumentHighlightKind)>> {
    let path = String::from("file://") + parsed_file.path.as_str();
    let uri = Url::parse(path.as_str())?;
    let mut refs = vec![];
    for r in &parsed_file.workspace.references {
        let r_ref = r.borrow();
        if let ReferenceTarget::Namespace(v) = &r_ref.target {
            if Arc::ptr_eq(&ns, v) {
                let location = Location::new(uri.clone(), r_ref.loc.into());
                refs.push((location, DocumentHighlightKind::TEXT));
            }
        }
    }
    Ok(refs)
}

fn find_references_to_field<'a>(
    parsed_file: &AtomicRefMut<'_, ParsedFile>,
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
                    if Arc::ptr_eq(&base_def, &def) {
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
    parsed_file: &AtomicRefMut<'_, ParsedFile>,
    pos: Point,
) -> Option<Arc<AtomicRefCell<VariableDefinition>>> {
    if let Some(tree) = &parsed_file.tree {
        let root = tree.root_node();
        if let Some(node) = root.descendant_for_point_range(pos, pos) {
            if let Some(parent) = node.parent() {
                let pos = parent.start_position();
                for r in &parsed_file.workspace.references {
                    let r_ref = r.borrow();
                    if r_ref.loc.contains(pos) {
                        if let ReferenceTarget::Variable(v) = &r_ref.target {
                            return Some(Arc::clone(v));
                        }
                    }
                }
                for d in &parsed_file.workspace.variables {
                    if d.borrow().loc.contains(pos) {
                        return Some(Arc::clone(d));
                    }
                }
            }
        }
    }
    None
}
