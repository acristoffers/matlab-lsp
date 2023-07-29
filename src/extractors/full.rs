/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::sync::Arc;

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use log::error;
use lsp_server::Message;

use crate::threads::db::{
    db_delete_file_function, db_fetch_parsed_files, db_set_function, db_set_packages,
    db_set_parsed_file,
};
use crate::types::{ParsedFile, SenderThread, ThreadMessage};
use crate::utils::{send_progress_begin, send_progress_end, send_progress_report};

use super::fast::{parse, traverse_folder};
use super::symbols::extract_symbols;

pub fn full_scan(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    folders: Vec<String>,
    id: i32,
) -> Result<()> {
    let mut folders = folders;
    folders.sort();
    folders.dedup();
    let mut files = vec![];
    let mut packages = vec![];
    for folder in folders {
        let (fs, ps) = traverse_folder(folder.clone(), String::new());
        files.extend(fs);
        packages.extend(ps);
    }
    db_set_packages(&sender, packages, SenderThread::BackgroundWorker)?;
    send_progress_begin(
        lsp_sender.clone(),
        id,
        "Scanning workspace.",
        format!("0/{}", files.len()),
    )?;
    for (i, (pkg, path)) in files.iter().enumerate() {
        if let Ok((file, fun)) = parse(pkg.clone(), path.clone()) {
            db_delete_file_function(&sender, path.clone(), SenderThread::BackgroundWorker)?;
            if let Some(fun) = fun {
                db_set_function(&sender, Arc::new(fun), SenderThread::BackgroundWorker)?;
            }
            match extract_symbols(
                sender.clone(),
                receiver.clone(),
                SenderThread::BackgroundWorker,
                Arc::new(file),
            ) {
                Ok(file) => db_set_parsed_file(&sender, file, SenderThread::BackgroundWorker)?,
                Err(err) => error!("Error analyzing file: {err:?}"),
            }
        }
        send_progress_report(
            lsp_sender.clone(),
            id,
            "Scanning workspace.",
            (100 * i / files.len()).try_into()?,
        )?;
    }
    send_progress_end(lsp_sender.clone(), id, "Finished scanning workspace.")?;
    sender.send(ThreadMessage {
        sender: SenderThread::BackgroundWorker,
        payload: crate::types::MessagePayload::ScanOpen,
    })?;
    Ok(())
}

pub fn scan_open(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
    receiver: Receiver<ThreadMessage>,
    id: i32,
) -> Result<()> {
    if let Some(open_files) = db_fetch_parsed_files(&sender, &receiver, SenderThread::Handler) {
        let files: Vec<Arc<ParsedFile>> = open_files
            .values()
            .filter(|f| f.open)
            .map(Arc::clone)
            .collect();
        send_progress_begin(
            lsp_sender.clone(),
            id,
            "Scanning open files.",
            format!("0/{}", files.len()),
        )?;
        for (i, file) in files.iter().enumerate() {
            let file = extract_symbols(
                sender.clone(),
                receiver.clone(),
                SenderThread::Handler,
                Arc::clone(file),
            )?;
            db_set_parsed_file(&sender, file, SenderThread::Handler)?;
            send_progress_report(
                lsp_sender.clone(),
                id,
                "Scanning open files.",
                (100 * i / files.len()).try_into()?,
            )?;
        }
        send_progress_end(lsp_sender.clone(), id, "Finished scanning open files.")?;
    }
    Ok(())
}
