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
use lsp_server::Message;
use lsp_types::InitializeParams;

use crate::parsed_file::ParsedFile;
use crate::utils::{lock_mutex, SessionStateArc};

pub struct SessionState {
    // Workspace and editor parsed files
    pub files: HashMap<String, ParsedFile>,

    pub workspace: InitializeParams,

    // Server connection
    pub sender: Sender<Message>,

    // Extra folders to analyze
    pub path: Vec<String>,
}

impl SessionState {
    pub fn parse_path_async(arc: SessionStateArc) -> Result<Vec<JoinHandle<Result<()>>>> {
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
            let state = Arc::clone(&arc);
            let handle = spawn(move || -> Result<()> {
                let dir = std::fs::read_dir(path)?;
                for entry in dir.flatten() {
                    if entry.metadata()?.is_file() {
                        let path = entry.path().to_string_lossy().to_string();
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
