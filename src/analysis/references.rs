/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{anyhow, Result};
use log::debug;
use lsp_types::{Location, Url};
use tree_sitter::Point;

use crate::code_loc;
use crate::parsed_file::ParsedFile;
use crate::session_state::{Namespace, SessionState};
use crate::types::{ClassDefinition, FunctionDefinition, ReferenceTarget, VariableDefinition};
use crate::utils::lock_mutex;

pub fn find_references_to_symbol<'a>(
    state: &'a MutexGuard<'a, &mut SessionState>,
    path: String,
    loc: Point,
    inc_dec: bool,
) -> Result<Vec<Location>> {
    debug!("Listing references.");
    let file = state.files.get(&path).ok_or(code_loc!("No such file."))?;
    let file_lock = lock_mutex(file)?;
    for r in &file_lock.workspace.references {
        let r_lock = lock_mutex(r)?;
        if r_lock.loc.contains(loc) {
            let target = r_lock.target.clone();
            match &target {
                ReferenceTarget::Class(c) => {
                    drop(r_lock);
                    drop(file_lock);
                    return find_references_to_class(state, Arc::clone(c), inc_dec);
                }
                ReferenceTarget::Function(f) => {
                    drop(r_lock);
                    drop(file_lock);
                    return find_references_to_function(state, Arc::clone(f), inc_dec);
                }
                ReferenceTarget::Script(f) => {
                    drop(r_lock);
                    drop(file_lock);
                    return find_references_to_script(state, Arc::clone(f));
                }
                ReferenceTarget::Variable(v) => {
                    drop(r_lock);
                    return find_references_to_variable(&file_lock, Arc::clone(v), inc_dec);
                }
                ReferenceTarget::UnknownVariable => {
                    let name = r_lock.name.clone();
                    drop(r_lock);
                    if name.contains('.') {
                        return find_references_to_field(&file_lock, name, loc);
                    }
                    return Ok(vec![]);
                }
                ReferenceTarget::Namespace(ns) => {
                    drop(r_lock);
                    return find_references_to_namespace(&file_lock, Arc::clone(ns));
                }
                _ => return Ok(vec![]),
            }
        }
    }
    for v in &file_lock.workspace.variables {
        let v_lock = lock_mutex(v)?;
        if v_lock.loc.contains(loc) {
            drop(v_lock);
            return find_references_to_variable(&file_lock, Arc::clone(v), inc_dec);
        }
    }
    for f in file_lock.workspace.functions.values() {
        let f_lock = lock_mutex(f)?;
        if f_lock.loc.contains(loc) {
            drop(f_lock);
            return find_references_to_function(state, Arc::clone(f), inc_dec);
        }
    }
    for c in file_lock.workspace.classes.values() {
        let c_lock = lock_mutex(c)?;
        if c_lock.loc.contains(loc) {
            drop(c_lock);
            return find_references_to_class(state, Arc::clone(c), inc_dec);
        }
    }
    Ok(vec![])
}

fn find_references_to_class<'a>(
    state: &'a MutexGuard<'a, &mut SessionState>,
    class: Arc<Mutex<ClassDefinition>>,
    inc_dec: bool,
) -> Result<Vec<Location>> {
    let mut refs = vec![];
    for (path, file) in &state.files {
        let lock = lock_mutex(file)?;
        let f_refs = lock.workspace.references.iter().map(|r| (path.clone(), r));
        for (r_path, reference) in f_refs {
            let r_lock = lock_mutex(reference)?;
            if let ReferenceTarget::Class(target) = &r_lock.target {
                if Arc::ptr_eq(&class, target) {
                    let path = String::from("file://") + r_path.as_str();
                    let uri = Url::parse(path.as_str())?;
                    let location = Location::new(uri.clone(), r_lock.loc.into());
                    refs.push(location);
                }
            }
        }
    }
    if inc_dec {
        let v_lock = lock_mutex(&class)?;
        let v_file_lock = lock_mutex(&v_lock.parsed_file)?;
        let path = v_file_lock.path.clone();
        let path = String::from("file://") + path.as_str();
        let uri = Url::parse(path.as_str())?;
        let loc = v_lock.loc;
        let location = Location::new(uri.clone(), loc.into());
        refs.push(location);
    }
    Ok(refs)
}

fn find_references_to_function<'a>(
    state: &'a MutexGuard<'a, &mut SessionState>,
    function: Arc<Mutex<FunctionDefinition>>,
    inc_dec: bool,
) -> Result<Vec<Location>> {
    let mut refs = vec![];
    for (path, file) in &state.files {
        let lock = lock_mutex(file)?;
        let f_refs = lock.workspace.references.iter().map(|r| (path.clone(), r));
        for (r_path, reference) in f_refs {
            let r_lock = lock_mutex(reference)?;
            if let ReferenceTarget::Function(target) = &r_lock.target {
                if Arc::ptr_eq(&function, target) {
                    let path = String::from("file://") + r_path.as_str();
                    let uri = Url::parse(path.as_str())?;
                    let location = Location::new(uri.clone(), r_lock.loc.into());
                    refs.push(location);
                }
            }
        }
    }
    if inc_dec {
        let v_lock = lock_mutex(&function)?;
        let v_file_lock = lock_mutex(&v_lock.parsed_file)?;
        let path = v_file_lock.path.clone();
        let path = String::from("file://") + path.as_str();
        let uri = Url::parse(path.as_str())?;
        let loc = v_lock.loc;
        let location = Location::new(uri.clone(), loc.into());
        refs.push(location);
    }
    Ok(refs)
}

fn find_references_to_script<'a>(
    state: &'a MutexGuard<'a, &mut SessionState>,
    script: Arc<Mutex<ParsedFile>>,
) -> Result<Vec<Location>> {
    let mut refs = vec![];
    for (path, file) in &state.files {
        let lock = lock_mutex(file)?;
        let f_refs = lock.workspace.references.iter().map(|r| (path.clone(), r));
        for (r_path, reference) in f_refs {
            let r_lock = lock_mutex(reference)?;
            if let ReferenceTarget::Script(target) = &r_lock.target {
                if Arc::ptr_eq(&script, target) {
                    let path = String::from("file://") + r_path.as_str();
                    let uri = Url::parse(path.as_str())?;
                    let location = Location::new(uri.clone(), r_lock.loc.into());
                    refs.push(location);
                }
            }
        }
    }
    Ok(refs)
}

fn find_references_to_variable<'a>(
    parsed_file: &'a MutexGuard<'a, ParsedFile>,
    variable: Arc<Mutex<VariableDefinition>>,
    inc_dec: bool,
) -> Result<Vec<Location>> {
    let path = String::from("file://") + parsed_file.path.as_str();
    let uri = Url::parse(path.as_str())?;
    let mut refs = vec![];
    for r in &parsed_file.workspace.references {
        let r_lock = lock_mutex(r)?;
        if let ReferenceTarget::Variable(v) = &r_lock.target {
            if Arc::ptr_eq(&variable, v) {
                let location = Location::new(uri.clone(), r_lock.loc.into());
                refs.push(location);
            }
        }
    }
    if inc_dec {
        let v_lock = lock_mutex(&variable)?;
        let loc = v_lock.loc;
        let location = Location::new(uri.clone(), loc.into());
        refs.push(location);
    }
    Ok(refs)
}

fn find_references_to_namespace<'a>(
    parsed_file: &'a MutexGuard<'a, ParsedFile>,
    ns: Arc<Mutex<Namespace>>,
) -> Result<Vec<Location>> {
    let path = String::from("file://") + parsed_file.path.as_str();
    let uri = Url::parse(path.as_str())?;
    let mut refs = vec![];
    for r in &parsed_file.workspace.references {
        let r_lock = lock_mutex(r)?;
        if let ReferenceTarget::Namespace(v) = &r_lock.target {
            if Arc::ptr_eq(&ns, v) {
                let location = Location::new(uri.clone(), r_lock.loc.into());
                refs.push(location);
            }
        }
    }
    Ok(refs)
}

fn find_references_to_field<'a>(
    parsed_file: &'a MutexGuard<'a, ParsedFile>,
    name: String,
    pos: Point,
) -> Result<Vec<Location>> {
    let path = String::from("file://") + parsed_file.path.as_str();
    let uri = Url::parse(path.as_str())?;
    let mut rs = vec![];
    if let Some(base_def) = base_definition(parsed_file, pos) {
        for r in &parsed_file.workspace.references {
            let r_lock = lock_mutex(r)?;
            if r_lock.name == name {
                let range = r_lock.loc;
                let pos = r_lock.loc.start;
                drop(r_lock);
                if let Some(def) = base_definition(parsed_file, pos) {
                    if Arc::ptr_eq(&base_def, &def) {
                        let location = Location::new(uri.clone(), range.into());
                        rs.push(location);
                    }
                }
            }
        }
    }
    Ok(rs)
}

fn base_definition(
    parsed_file: &MutexGuard<'_, ParsedFile>,
    pos: Point,
) -> Option<Arc<Mutex<VariableDefinition>>> {
    if let Some(tree) = &parsed_file.tree {
        let root = tree.root_node();
        if let Some(node) = root.descendant_for_point_range(pos, pos) {
            if let Some(parent) = node.parent() {
                let pos = parent.start_position();
                for r in &parsed_file.workspace.references {
                    if let Ok(r_lock) = lock_mutex(r) {
                        if r_lock.loc.contains(pos) {
                            if let ReferenceTarget::Variable(v) = &r_lock.target {
                                return Some(Arc::clone(v));
                            }
                        }
                    }
                }
                for d in &parsed_file.workspace.variables {
                    if let Ok(d_lock) = lock_mutex(d) {
                        if d_lock.loc.contains(pos) {
                            return Some(Arc::clone(d));
                        }
                    }
                }
            }
        }
    }
    None
}
