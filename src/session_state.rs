/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;
use std::sync::Arc;
use std::thread::{spawn, JoinHandle};

use anyhow::Result;
use log::info;

use crate::parsed_file::ParsedFile;
pub use crate::types::{ClassFolder, Namespace, SessionState};
use crate::utils::{lock_mutex, SessionStateArc};

impl SessionState {
    pub fn parse_path_async(arc: SessionStateArc) -> Result<Vec<JoinHandle<Result<()>>>> {
        info!("Scanning workspace and path");
        let lock = lock_mutex(&arc)?;
        let mut paths = lock.path.clone();
        if let Some(uri) = &lock.workspace.root_uri {
            if let Some(path) = uri.as_str().strip_prefix("file://") {
                paths.push(path.into());
            }
        }
        if let Some(workspace_folders) = &lock.workspace.workspace_folders {
            for folder in workspace_folders {
                if let Some(path) = folder.uri.as_str().strip_prefix("file://") {
                    paths.push(path.into());
                }
            }
        }
        drop(lock);
        let mut handles = vec![];
        for path in paths {
            info!("Launching thread to scan {path}");
            let state = Arc::clone(&arc);
            let handle =
                spawn(move || -> Result<()> { SessionState::scan_folder(state, path, None, None) });
            handles.push(handle);
        }
        Ok(handles)
    }

    fn scan_folder(
        state: SessionStateArc,
        path: String,
        ns: Option<&mut Namespace>,
        cf: Option<&mut ClassFolder>,
    ) -> Result<()> {
        let dir = std::fs::read_dir(path)?;
        let mut ns = ns;
        let mut cf = cf;
        for entry in dir.flatten() {
            if entry.metadata()?.is_file() {
                if !entry.file_name().to_string_lossy().ends_with(".m") {
                    continue;
                }
                let path = entry.path().to_string_lossy().to_string();
                let url = String::from("file://") + path.as_str();
                let lock = lock_mutex(&state)?;
                if lock.files.contains_key(&url) {
                    continue;
                }
                drop(lock);
                let parsed_file = ParsedFile::parse_file(path)?;
                let path = parsed_file.file.as_str().to_string().clone();
                if let Some(ns) = ns.as_mut() {
                    ns.files.push(path.clone());
                }
                if let Some(ns) = ns.as_mut() {
                    ns.files.push(path.clone());
                } else if let Some(cf) = cf.as_mut() {
                    cf.files.push(path.clone());
                }
                lock_mutex(&state)?.files.insert(path, parsed_file);
            } else if entry.metadata()?.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                let state = Arc::clone(&state);
                let path = entry.path().to_string_lossy().to_string();
                if name.starts_with('+') {
                    let mut namespace = Namespace {
                        name: name.strip_prefix('+').unwrap().into(),
                        files: vec![],
                        namespaces: HashMap::new(),
                        classes: HashMap::new(),
                    };
                    SessionState::scan_folder(
                        Arc::clone(&state),
                        path.clone(),
                        Some(&mut namespace),
                        None,
                    )?;
                    if let Some(ns) = ns.as_mut() {
                        ns.namespaces.insert(namespace.name.clone(), namespace);
                    } else if let Some(cf) = cf.as_mut() {
                        cf.namespaces.insert(namespace.name.clone(), namespace);
                    } else {
                        lock_mutex(&state)?
                            .namespaces
                            .insert(namespace.name.clone(), namespace);
                    }
                } else if name.starts_with('@') {
                    let mut class_folder = ClassFolder {
                        name: name.strip_prefix('@').unwrap().into(),
                        files: vec![],
                        namespaces: HashMap::new(),
                        classes: HashMap::new(),
                    };
                    SessionState::scan_folder(
                        Arc::clone(&state),
                        path.clone(),
                        None,
                        Some(&mut class_folder),
                    )?;
                    if let Some(ns) = ns.as_mut() {
                        ns.classes.insert(class_folder.name.clone(), class_folder);
                    } else if let Some(cf) = cf.as_mut() {
                        cf.classes.insert(class_folder.name.clone(), class_folder);
                    } else {
                        lock_mutex(&state)?
                            .classes
                            .insert(class_folder.name.clone(), class_folder);
                    }
                }
            }
        }
        Ok(())
    }
}
