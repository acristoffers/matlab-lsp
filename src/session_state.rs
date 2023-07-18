/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;
use std::sync::{Arc, MutexGuard};
use std::thread::{spawn, JoinHandle};

use anyhow::{anyhow, Context, Result};
use atomic_refcell::AtomicRefCell;
use log::{debug, error, info};
use lsp_server::{Message, RequestId};
use lsp_types::request::{Request, SemanticTokensRefresh};

use crate::analysis::diagnostics;
use crate::code_loc;
use crate::parsed_file::{FileType, ParsedFile};
pub use crate::types::{ClassFolder, Namespace, SessionState};
use crate::utils::{
    lock_mutex, rescan_file, send_progress_begin, send_progress_end, send_progress_report,
    SessionStateArc,
};

type FileTuple = (
    String,
    Option<Arc<AtomicRefCell<Namespace>>>,
    Option<Arc<AtomicRefCell<ClassFolder>>>,
);

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
                }
                if lock.rescan_all_files {
                    debug!("Rescanning all files.");
                    if let Err(err) = SessionState::rescan_all_files(&mut lock) {
                        error!("Error scanning all files: {err}");
                    }
                    lock.rescan_all_files = false;
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
        send_progress_begin(
            state,
            0,
            "Rescanning files.",
            format!("0/{}", state.files.len()),
        )?;
        for (i, file) in state.files.clone().values().enumerate() {
            if file.borrow().open {
                let file2 = Arc::clone(file);
                rescan_file(state, file2)?;
                let pf_mr = file.borrow_mut();
                diagnostics::diagnotiscs(state, &pf_mr)?;
                send_progress_report(
                    state,
                    0,
                    format!("scanning file {}/{}", i, state.files.len()),
                    (100 * i / state.files.len()).try_into()?,
                )?;
            }
        }
        send_progress_end(state, 0, "Files rescanned.")?;
        state.sender.send(Message::Request(lsp_server::Request {
            id: RequestId::from(state.request_id),
            method: SemanticTokensRefresh::METHOD.to_string(),
            params: serde_json::to_value(())?,
        }))?;
        state.request_id += 1;
        Ok(())
    }

    fn rescan_all_files(state: &mut MutexGuard<'_, &mut SessionState>) -> Result<()> {
        debug!("Rescanning project files.");
        let project_path = if let Some(uri) = &state.workspace_params.root_uri {
            uri.path().to_owned()
        } else {
            "".to_owned()
        };
        info!("Root URI while rescanning: {project_path}");
        state
            .workspace
            .classes
            .retain(|k, _| !k.starts_with(&project_path));
        state
            .workspace
            .functions
            .retain(|k, _| !k.starts_with(&project_path));
        state
            .workspace
            .scripts
            .retain(|k, _| !k.starts_with(&project_path));
        state.workspace.references.clear();
        state.workspace.variables.clear();
        send_progress_begin(
            state,
            0,
            "Rescanning files.",
            format!("0/{}", state.files.len()),
        )?;
        // Twice, to get cross-references right.
        for _ in 0..2 {
            for (i, file) in state.files.clone().values().enumerate() {
                let file = Arc::clone(file);
                if !file.borrow().path.starts_with(&project_path) {
                    continue;
                }
                rescan_file(state, file)?;
                send_progress_report(
                    state,
                    0,
                    format!("scanning file {}/{}", i, state.files.len()),
                    (100 * i / state.files.len()).try_into()?,
                )?;
            }
        }
        send_progress_end(state, 0, "Files rescanned.")?;
        state.sender.send(Message::Request(lsp_server::Request {
            id: RequestId::from(state.request_id),
            method: SemanticTokensRefresh::METHOD.to_string(),
            params: serde_json::to_value(())?,
        }))?;
        state.request_id += 1;
        Ok(())
    }

    fn full_scan_path(state: SessionStateArc) -> Result<()> {
        info!("Scanning workspace and path");
        let mut lock = lock_mutex(&state).context(code_loc!())?;
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
        paths.sort();
        paths.dedup();
        let mut files = vec![];
        for path in paths {
            let state = Arc::clone(&state);
            let fs = SessionState::scan_folder(state, path.clone(), None, None)?;
            files.extend(fs);
        }
        let mut lock = lock_mutex(&state).context(code_loc!())?;
        send_progress_begin(
            &mut lock,
            0,
            "Parsing workspace files:",
            format!("0/{} files parsed.", files.len()),
        )?;
        drop(lock);
        for (i, (path, ns, cs)) in files.iter().enumerate() {
            let mut lock = lock_mutex(&state).context(code_loc!())?;
            send_progress_report(
                &mut lock,
                0,
                format!("{}/{} files parsed.", i, files.len()),
                (100 * i / files.len()).try_into()?,
            )?;
            SessionState::parse_file(&mut lock, path.clone(), ns.clone(), cs.clone())?;
            drop(lock); // Gives the main thread some time to process too.
        }
        let mut lock = lock_mutex(&state).context(code_loc!())?;
        send_progress_end(&mut lock, 0, "Finished parsing files.")?;
        Ok(())
    }

    fn scan_folder(
        state: SessionStateArc,
        path: String,
        ns: Option<Arc<AtomicRefCell<Namespace>>>,
        cf: Option<Arc<AtomicRefCell<ClassFolder>>>,
    ) -> Result<Vec<FileTuple>> {
        let dir = std::fs::read_dir(path.clone()).context(code_loc!())?;
        let mut ns = ns;
        let mut cf = cf;
        let mut files = vec![];
        for entry in dir.flatten() {
            debug!("Considering entry {:?}", entry);
            if entry.metadata().context(code_loc!())?.is_file() {
                if !entry.file_name().to_string_lossy().ends_with(".m") {
                    continue;
                }
                let path = entry.path().to_string_lossy().to_string();
                let ns = if let Some(n) = ns.take() {
                    ns = Some(Arc::clone(&n));
                    Some(n)
                } else {
                    None
                };
                let cf = if let Some(c) = cf.take() {
                    cf = Some(Arc::clone(&c));
                    Some(c)
                } else {
                    None
                };
                files.push((path, ns, cf));
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
                        ns.borrow_mut().path.clone() + "." + name.as_str()
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
                    let namespace = Arc::new(AtomicRefCell::new(namespace));
                    let fs = SessionState::scan_folder(
                        Arc::clone(&state),
                        folder_path.clone(),
                        Some(Arc::clone(&namespace)),
                        None,
                    )
                    .context(code_loc!())?;
                    files.extend(fs);
                    if let Some(ns) = ns.as_ref() {
                        ns.borrow_mut()
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
                        ns.borrow_mut().path.clone() + "." + name.as_str()
                    } else {
                        name.clone()
                    };
                    let class_folder = ClassFolder {
                        name: name.clone(),
                        path: folder_path.clone(),
                        files: vec![],
                        methods: vec![],
                    };
                    let class_folder = Arc::new(AtomicRefCell::new(class_folder));
                    let fs = SessionState::scan_folder(
                        Arc::clone(&state),
                        folder_path.clone(),
                        None,
                        Some(Arc::clone(&class_folder)),
                    )
                    .context(code_loc!())?;
                    files.extend(fs);
                    if let Some(ns) = ns.as_mut() {
                        ns.borrow_mut()
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
        Ok(files)
    }

    fn parse_file(
        state: &mut MutexGuard<'_, &mut SessionState>,
        path: String,
        ns: Option<Arc<AtomicRefCell<Namespace>>>,
        cf: Option<Arc<AtomicRefCell<ClassFolder>>>,
    ) -> Result<()> {
        let mut ns = ns;
        let cf = cf;
        if state.files.contains_key(&path) {
            debug!("File {path} is already parsed, skipping");
            return Ok(());
        }
        debug!("Working on file {path}");
        let parsed_file = ParsedFile::parse_file(path).context(code_loc!())?;
        debug!(
            "Parsed file contains {} bytes of code.",
            parsed_file.contents.len()
        );
        let parsed_file = Arc::new(AtomicRefCell::new(parsed_file));
        let ns_path = if let Some(ns_) = ns {
            ns = Some(Arc::clone(&ns_));
            ns_.borrow_mut().path.clone()
        } else {
            "".into()
        };
        let mut pf_mr = parsed_file.borrow_mut();
        let path = pf_mr.path.clone();
        if let Some(ns) = &ns {
            let mut ns_mr = ns.borrow_mut();
            ns_mr.files.push(Arc::clone(&parsed_file));
            pf_mr
                .workspace
                .namespaces
                .insert(ns_mr.path.clone(), Arc::clone(ns));
            pf_mr.in_namespace = Some(Arc::clone(ns));
        } else if let Some(cf) = &cf {
            let mut cf_mr = cf.borrow_mut();
            cf_mr.files.push(Arc::clone(&parsed_file));
            pf_mr
                .workspace
                .classfolders
                .insert(cf_mr.path.clone(), Arc::clone(cf));
            pf_mr.in_classfolder = Some(Arc::clone(cf));
        }
        debug!("Inserting file {path} into state.");
        ParsedFile::define_type(state, Arc::clone(&parsed_file), &mut pf_mr, ns_path)?;
        state.files.insert(path, Arc::clone(&parsed_file));
        match &pf_mr.file_type {
            FileType::Function(f) => {
                state
                    .workspace
                    .functions
                    .insert(f.borrow_mut().path.clone(), Arc::clone(f));
            }
            FileType::Class(f) => {
                state
                    .workspace
                    .classes
                    .insert(f.borrow_mut().path.clone(), Arc::clone(f));
            }
            _ => {}
        }
        drop(pf_mr);
        Ok(())
    }
}
