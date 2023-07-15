/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{spawn, JoinHandle};

use anyhow::{anyhow, Context, Result};
use log::{debug, error, info};

use crate::analysis::defref;
use crate::code_loc;
use crate::parsed_file::{FileType, ParsedFile};
pub use crate::types::{ClassFolder, Namespace, SessionState};
use crate::utils::{lock_mutex, rescan_file, SessionStateArc};

impl SessionState {
    pub fn start_worker(state: SessionStateArc) -> Result<JoinHandle<Result<()>>> {
        let handle = spawn(move || -> Result<()> {
            SessionState::full_scan_path(Arc::clone(&state))?;
            loop {
                let mut lock = lock_mutex(&state)?;
                if lock.client_requested_shutdown {
                    debug!("Shutdown requested, leaving thread.");
                    break;
                }
                if lock.rescan_open_files {
                    debug!("Rescanning open files.");
                    if let Err(err) = SessionState::rescan_open_files(&mut lock) {
                        error!("Error scanning open files: {err}");
                    }
                    lock.rescan_open_files = false;
                    continue;
                }
                if lock.rescan_all_files {
                    debug!("Rescanning all files.");
                    if let Err(err) = SessionState::rescan_all_files(&mut lock) {
                        error!("Error scanning all files: {err}");
                    }
                    lock.rescan_all_files = false;
                    continue;
                }
                drop(lock);
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            debug!("Leaving worker thread.");
            Ok(())
        });
        Ok(handle)
    }

    fn rescan_open_files(state: &mut MutexGuard<'_, &mut SessionState>) -> Result<()> {
        debug!("Rescanning open files.");
        for file in state.files.clone().values() {
            let file_lock = lock_mutex(file)?;
            if file_lock.open {
                drop(file_lock);
                let file = Arc::clone(file);
                rescan_file(state, file)?;
            }
        }
        Ok(())
    }

    fn rescan_all_files(state: &mut MutexGuard<'_, &mut SessionState>) -> Result<()> {
        debug!("Rescanning open files.");
        state.workspace.classes.clear();
        state.workspace.functions.clear();
        state.workspace.references.clear();
        state.workspace.scripts.clear();
        state.workspace.variables.clear();
        // Twice, to get cross-references right.
        for _ in 0..2 {
            for file in state.files.clone().values() {
                let file = Arc::clone(file);
                rescan_file(state, file)?;
            }
        }
        Ok(())
    }

    fn full_scan_path(arc: SessionStateArc) -> Result<()> {
        info!("Scanning workspace and path");
        let mut lock = lock_mutex(&arc).context(code_loc!())?;
        let mut paths = lock.path.clone();
        if let Some(uri) = &lock.workspace_params.root_uri {
            let path = uri.path().to_string();
            paths.push(path);
        }
        if let Some(fs) = &lock.workspace_params.workspace_folders {
            let other: Vec<String> = fs.iter().map(|f| f.uri.path().to_string()).collect();
            paths.extend(other);
        }
        lock.rescan_open_files = true;
        drop(lock);
        let mut handles = vec![];
        paths.sort();
        paths.dedup();
        for path in paths {
            info!("Launching thread to scan {path}");
            let state = Arc::clone(&arc);
            let handle = spawn(move || -> Result<()> {
                info!("Thread started for {}", path);
                if let Err(err) = SessionState::scan_folder(state, path.clone(), None, None) {
                    error!("Error scanning path {}: {}", path, err);
                    Err(err)
                } else {
                    info!("Thread for path {} finished.", path);
                    Ok(())
                }
            });
            handles.push(handle);
        }
        for handle in handles {
            let _ = handle.join();
        }
        Ok(())
    }

    fn scan_folder(
        state: SessionStateArc,
        path: String,
        ns: Option<Arc<Mutex<Namespace>>>,
        cf: Option<Arc<Mutex<ClassFolder>>>,
    ) -> Result<()> {
        let dir = std::fs::read_dir(path.clone()).context(code_loc!())?;
        let mut ns = ns;
        let cf = cf;
        for entry in dir.flatten() {
            debug!("Considering entry {:?}", entry);
            if entry.metadata().context(code_loc!())?.is_file() {
                if !entry.file_name().to_string_lossy().ends_with(".m") {
                    continue;
                }
                let path = entry.path().to_string_lossy().to_string();
                let lock = lock_mutex(&state).context(code_loc!())?;
                if lock.files.contains_key(&path) {
                    debug!("File {path} is already parsed, skipping");
                    continue;
                }
                debug!("Working on file {path}");
                drop(lock);
                let parsed_file = ParsedFile::parse_file(path).context(code_loc!())?;
                debug!(
                    "Parsed file contains {} bytes of code.",
                    parsed_file.contents.len()
                );
                let parsed_file = Arc::new(Mutex::new(parsed_file));
                let ns_path = if let Some(ns_) = ns {
                    ns = Some(Arc::clone(&ns_));
                    lock_mutex(&ns_)?.path.clone()
                } else {
                    "".into()
                };
                let mut parsed_file_lock = lock_mutex(&parsed_file)?;
                let path = parsed_file_lock.path.clone();
                if let Some(ns) = &ns {
                    let mut ns_lock = lock_mutex(ns)?;
                    ns_lock.files.push(Arc::clone(&parsed_file));
                    parsed_file_lock
                        .workspace
                        .namespaces
                        .insert(ns_lock.path.clone(), Arc::clone(ns));
                    parsed_file_lock.in_namespace = Some(Arc::clone(ns));
                } else if let Some(cf) = &cf {
                    let mut cf_lock = lock_mutex(cf)?;
                    cf_lock.files.push(Arc::clone(&parsed_file));
                    parsed_file_lock
                        .workspace
                        .classfolders
                        .insert(cf_lock.path.clone(), Arc::clone(cf));
                    parsed_file_lock.in_classfolder = Some(Arc::clone(cf));
                }
                debug!("Inserting file {path} into state.");
                let mut lock = lock_mutex(&state).context(code_loc!())?;
                ParsedFile::define_type(
                    &mut lock,
                    Arc::clone(&parsed_file),
                    &mut parsed_file_lock,
                    ns_path,
                )?;
                lock.files.insert(path, Arc::clone(&parsed_file));
                match &parsed_file_lock.file_type {
                    FileType::Function(f) => {
                        lock.workspace
                            .functions
                            .insert(lock_mutex(f)?.path.clone(), Arc::clone(f));
                    }
                    FileType::Class(f) => {
                        lock.workspace
                            .classes
                            .insert(lock_mutex(f)?.path.clone(), Arc::clone(f));
                    }
                    _ => {}
                }
                drop(parsed_file_lock);
                defref::analyze(&lock, Arc::clone(&parsed_file))?;
            } else if entry.metadata().context(code_loc!())?.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                let folder_path = entry.path().to_string_lossy().to_string();
                if name.starts_with('+') {
                    debug!("Adding package {path}");
                    let name = name
                        .strip_prefix('+')
                        .ok_or(anyhow!("Could not remove + prefix"))
                        .context(code_loc!())?
                        .to_string();
                    let path = if let Some(ns) = ns.as_mut() {
                        lock_mutex(ns)?.path.clone() + "." + name.as_str()
                    } else {
                        name.clone()
                    };
                    let namespace = Namespace {
                        name: name.clone(),
                        path: path.clone(),
                        files: vec![],
                        namespaces: HashMap::new(),
                        classfolders: HashMap::new(),
                        functions: HashMap::new(),
                        classes: HashMap::new(),
                    };
                    let namespace = Arc::new(Mutex::new(namespace));
                    SessionState::scan_folder(
                        Arc::clone(&state),
                        folder_path.clone(),
                        Some(Arc::clone(&namespace)),
                        None,
                    )
                    .context(code_loc!())?;
                    if let Some(ns) = ns.as_ref() {
                        lock_mutex(ns)?
                            .namespaces
                            .insert(name.clone(), Arc::clone(&namespace));
                    }
                    lock_mutex(&state)
                        .context(code_loc!())?
                        .workspace
                        .namespaces
                        .insert(path.clone(), Arc::clone(&namespace));
                } else if name.starts_with('@') {
                    debug!("Adding class folder {path}");
                    let name = name
                        .strip_prefix('@')
                        .ok_or(anyhow!("Could not remove @ prefix"))
                        .context(code_loc!())?
                        .to_string();
                    let path = if let Some(ns) = ns.as_mut() {
                        lock_mutex(ns)?.path.clone() + "." + name.as_str()
                    } else {
                        name.clone()
                    };
                    let class_folder = ClassFolder {
                        name: name.clone(),
                        path: folder_path.clone(),
                        files: vec![],
                        methods: vec![],
                    };
                    let class_folder = Arc::new(Mutex::new(class_folder));
                    SessionState::scan_folder(
                        Arc::clone(&state),
                        folder_path.clone(),
                        None,
                        Some(Arc::clone(&class_folder)),
                    )
                    .context(code_loc!())?;
                    if let Some(ns) = ns.as_mut() {
                        lock_mutex(ns)?
                            .classfolders
                            .insert(path.clone(), Arc::clone(&class_folder));
                    }
                    lock_mutex(&state)
                        .context(code_loc!())?
                        .workspace
                        .classfolders
                        .insert(path.clone(), Arc::clone(&class_folder));
                }
            }
        }
        Ok(())
    }
}
