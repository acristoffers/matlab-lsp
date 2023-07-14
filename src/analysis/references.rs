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
use crate::session_state::SessionState;
use crate::types::{ClassDefinition, FunctionDefinition, ReferenceTarget, VariableDefinition};
use crate::utils::lock_mutex;

pub fn find_references_to_symbol<'a>(
    state: &'a MutexGuard<'a, &mut SessionState>,
    path: String,
    loc: Point,
) -> Result<Vec<Location>> {
    debug!("Listing references.");
    let file = state.files.get(&path).ok_or(code_loc!("No such file."))?;
    let file_lock = lock_mutex(file)?;
    for r in &file_lock.workspace.references {
        let r_lock = lock_mutex(r)?;
        if r_lock.loc.contains(loc) {
            let target = r_lock.target.clone();
            match &target {
                crate::types::ReferenceTarget::Class(c) => {
                    drop(r_lock);
                    drop(file_lock);
                    return find_references_to_class(state, Arc::clone(c));
                }
                crate::types::ReferenceTarget::Function(f) => {
                    drop(r_lock);
                    drop(file_lock);
                    return find_references_to_function(state, Arc::clone(f));
                }
                crate::types::ReferenceTarget::Script(f) => {
                    drop(r_lock);
                    drop(file_lock);
                    return find_references_to_script(state, Arc::clone(f));
                }
                crate::types::ReferenceTarget::Variable(v) => {
                    drop(r_lock);
                    return find_references_to_variable(&file_lock, Arc::clone(v));
                }
                _ => return Ok(vec![]),
            }
        }
    }
    for v in &file_lock.workspace.variables {
        let v_lock = lock_mutex(v)?;
        if v_lock.loc.contains(loc) {
            drop(v_lock);
            return find_references_to_variable(&file_lock, Arc::clone(v));
        }
    }
    for f in file_lock.workspace.functions.values() {
        let f_lock = lock_mutex(f)?;
        if f_lock.loc.contains(loc) {
            drop(f_lock);
            return find_references_to_function(state, Arc::clone(f));
        }
    }
    for c in file_lock.workspace.classes.values() {
        let c_lock = lock_mutex(c)?;
        if c_lock.loc.contains(loc) {
            drop(c_lock);
            return find_references_to_class(state, Arc::clone(c));
        }
    }
    Ok(vec![])
}

fn find_references_to_class<'a>(
    state: &'a MutexGuard<'a, &mut SessionState>,
    class: Arc<Mutex<ClassDefinition>>,
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
    Ok(refs)
}

fn find_references_to_function<'a>(
    state: &'a MutexGuard<'a, &mut SessionState>,
    function: Arc<Mutex<FunctionDefinition>>,
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
    Ok(refs)
}
