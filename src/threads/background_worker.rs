/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::extractors::fast::fast_scan;
use crate::extractors::full::full_scan;
use crate::threads::db::db_get_request_id;
use crate::types::{MessagePayload, SenderThread, ThreadMessage};
use crate::utils::request_semantic_tokens_refresh;
use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use log::{error, info};
use lsp_server::Message;

pub fn start(
    lsp_sender: Sender<Message>,
    dispatcher_sender: Sender<ThreadMessage>,
    dispatcher_receiver: Receiver<ThreadMessage>,
) -> Result<()> {
    loop {
        match dispatcher_receiver.recv()?.payload {
            MessagePayload::Exit => break,
            MessagePayload::ScanPath(path) => {
                if let Some(id) = db_get_request_id(
                    &dispatcher_sender,
                    &dispatcher_receiver,
                    SenderThread::BackgroundWorker,
                ) {
                    if let Err(err) =
                        fast_scan(lsp_sender.clone(), dispatcher_sender.clone(), path, id)
                    {
                        error!("Error scanning folders: {err}");
                    }
                }
            }
            MessagePayload::ScanWorkspace(path) => {
                if let Some(id) = db_get_request_id(
                    &dispatcher_sender,
                    &dispatcher_receiver,
                    SenderThread::BackgroundWorker,
                ) {
                    if let Err(err) = full_scan(
                        lsp_sender.clone(),
                        dispatcher_sender.clone(),
                        dispatcher_receiver.clone(),
                        path,
                        id,
                    ) {
                        error!("Error scanning workspace: {err}");
                    }
                }
            }
            _ => {}
        }
        request_semantic_tokens_refresh(
            &lsp_sender,
            &dispatcher_sender,
            &dispatcher_receiver,
            SenderThread::BackgroundWorker,
        )?;
        dispatcher_sender.send(ThreadMessage {
            sender: SenderThread::BackgroundWorker,
            payload: MessagePayload::Done,
        })?;
    }
    info!("Background Worker exited.");
    Ok(())
}
