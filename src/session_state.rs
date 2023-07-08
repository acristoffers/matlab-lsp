/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;
use std::sync::Arc;
use std::thread::{spawn, JoinHandle};

use anyhow::Result;
use crossbeam_channel::Sender;
use log::info;
use lsp_server::Message;
use lsp_types::InitializeParams;

use crate::parsed_file::ParsedFile;
use crate::utils::{lock_mutex, SessionStateArc};

pub struct SessionState {
    // Misc
    pub client_requested_shutdown: bool,
    pub path: Vec<String>,
    pub sender: Sender<Message>,
    pub workspace: InitializeParams,

    // Code states and structures
    pub files: HashMap<String, ParsedFile>,
}

impl SessionState {
    pub fn parse_path_async(arc: SessionStateArc) -> Result<Vec<JoinHandle<Result<()>>>> {
        info!("Scanning workspace and path");
        let mut paths = lock_mutex(&arc)?.path.clone();
        if let Some(uri) = &lock_mutex(&arc)?.workspace.root_uri {
            if let Some(path) = uri.as_str().strip_prefix("file://") {
                paths.push(path.into());
            }
        }
        if let Some(workspace_folders) = &lock_mutex(&arc)?.workspace.workspace_folders {
            for folder in workspace_folders {
                if let Some(path) = folder.uri.as_str().strip_prefix("file://") {
                    paths.push(path.into());
                }
            }
        }
        let mut handles = vec![];
        for path in paths {
            info!("Launching thread to scan {path}");
            let state = Arc::clone(&arc);
            let handle = spawn(move || -> Result<()> {
                let dir = std::fs::read_dir(path)?;
                for entry in dir
                    .flatten()
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".m"))
                {
                    if entry.metadata()?.is_file() {
                        let path = entry.path().to_string_lossy().to_string();
                        let url = String::from("file://") + path.as_str();
                        let lock = lock_mutex(&state)?;
                        if lock.files.contains_key(&url) {
                            continue;
                        }
                        drop(lock);
                        let parsed_file = ParsedFile::parse_file(path)?;
                        lock_mutex(&state)?
                            .files
                            .insert(parsed_file.file.as_str().into(), parsed_file);
                    }
                }
                Ok(())
            });
            handles.push(handle);
        }
        Ok(handles)
    }
}
