/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::Sender;
use itertools::Itertools;
use lsp_server::Message;
use tree_sitter::Node;

use crate::code_loc;
use crate::threads::db::db_set_packages;
use crate::types::{
    FunctionDefinition, FunctionSignature, MessagePayload, ParsedFile, Range, SenderThread,
    ThreadMessage,
};
use crate::utils::{send_progress_begin, send_progress_end, send_progress_report};

/// It's called a fast scan because it only extracts public information, so mostly function
/// definition. Those files are not analysed for symbols or anything.
pub fn fast_scan(
    lsp_sender: Sender<Message>,
    sender: Sender<ThreadMessage>,
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
    let mut parsed_files = vec![];
    let mut functions = vec![];
    send_progress_begin(
        lsp_sender.clone(),
        id,
        "Scanning files.",
        format!("0/{}", files.len()),
    )?;
    for (i, (pkg, path)) in files.iter().enumerate() {
        if let Ok((pf, fs)) = parse(pkg.clone(), path.clone()) {
            parsed_files.push(Arc::new(pf));
            if let Some(fs) = fs {
                functions.push(Arc::new(fs));
            }
        }
        send_progress_report(
            lsp_sender.clone(),
            id,
            "Scanning files.",
            (100 * i / files.len()).try_into()?,
        )?;
    }
    send_progress_end(lsp_sender.clone(), id, "Finished scanning files.")?;
    sender.send(ThreadMessage {
        sender: SenderThread::BackgroundWorker,
        payload: MessagePayload::InitPath((parsed_files, functions)),
    })?;
    Ok(())
}

pub fn traverse_folder(folder: String, package: String) -> (Vec<(String, String)>, Vec<String>) {
    let mut packages = vec![];
    let mut files = vec![];
    if let Ok(dir) = std::fs::read_dir(folder).context(code_loc!()) {
        for entry in dir.flatten() {
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_file() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".m") {
                        files.push((package.clone(), entry.path().to_string_lossy().to_string()))
                    }
                } else if metadata.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let path = entry.path().to_string_lossy().to_string();
                    if name.starts_with('+') {
                        let name = name.strip_prefix('+').unwrap();
                        let package_name = package.clone() + "." + name;
                        let package_name = package_name
                            .strip_prefix('.')
                            .map(String::from)
                            .unwrap_or(package_name);
                        packages.push(package_name.clone());
                        let (sub_files, sub_packages) =
                            traverse_folder(path.clone(), package_name.clone());
                        packages.extend(sub_packages);
                        files.extend(sub_files);
                    }
                }
            }
        }
    }
    (files, packages)
}

pub fn parse(package: String, path: String) -> Result<(ParsedFile, Option<FunctionDefinition>)> {
    let mut parsed_file = ParsedFile::new(path.clone(), None)?;
    parsed_file.package = package.clone();
    let function = public_function(&mut parsed_file);
    parsed_file.contents = String::new();
    Ok((parsed_file, function))
}

pub fn public_function(parsed_file: &mut ParsedFile) -> Option<FunctionDefinition> {
    let root = parsed_file.tree.root_node();
    let mut cursor = root.walk();
    let mut function = None;
    if let Some(node) = root
        .named_children(&mut cursor)
        .find(|n| n.kind() != "comment")
    {
        if node.kind() == "function_definition" {
            if let Ok(signature) = function_signature(parsed_file, node) {
                parsed_file.is_script = false;
                function = Some(FunctionDefinition {
                    loc: node.range().into(),
                    name: signature.name.clone(),
                    path: parsed_file.path.clone(),
                    signature,
                    package: parsed_file.package.clone(),
                });
            }
        } else if node.kind() == "class_definition" {
            parsed_file.is_script = false;
        }
    }
    drop(cursor);
    function
}

pub fn function_signature(parsed_file: &ParsedFile, node: Node) -> Result<FunctionSignature> {
    let (name, name_range) = if let Some(name) = node.child_by_field_name("name") {
        let name_range = name.range();
        let name = name.utf8_text(parsed_file.contents.as_bytes())?.to_string();
        (name, name_range)
    } else {
        return Err(anyhow!("Could not find function name"));
    };
    let mut sig_range: Range = node.range().into();
    sig_range.end = name_range.end_point;
    let mut cursor = node.walk();
    let mut argout: usize = 0;
    let mut vargout = false;
    let mut argout_names = vec![];
    if let Some(output) = node
        .named_children(&mut cursor)
        .find(|c| c.kind() == "function_output")
    {
        if let Some(args) = output.child(0) {
            if args.kind() == "identifier" {
                argout = 1;
                argout_names.push(args.utf8_text(parsed_file.contents.as_bytes())?.into());
            } else {
                argout = args.named_child_count();
                let mut cursor2 = args.walk();
                for arg_name in args
                    .named_children(&mut cursor2)
                    .filter(|c| c.kind() == "identifier")
                    .filter_map(|c| c.utf8_text(parsed_file.contents.as_bytes()).ok())
                    .map(String::from)
                {
                    if arg_name == "varargout" {
                        vargout = true;
                    } else {
                        argout_names.push(arg_name);
                    }
                }
                if vargout {
                    argout -= 1;
                }
            }
        }
    }
    let mut argin: usize = 0;
    let mut vargin = false;
    let mut argin_names = vec![];
    let mut vargin_names = vec![];
    if let Some(inputs) = node
        .named_children(&mut cursor)
        .find(|c| c.kind() == "function_arguments")
    {
        sig_range.end = inputs.end_position();
        argin = inputs.named_child_count();
        let mut cursor2 = node.walk();
        let mut cursor3 = node.walk();
        let mut cursor4 = node.walk();
        for arg_name in inputs
            .named_children(&mut cursor2)
            .filter_map(|c| c.utf8_text(parsed_file.contents.as_bytes()).ok())
            .map(String::from)
        {
            argin_names.push(arg_name);
        }
        let mut optional_arguments = HashMap::new();
        for argument in node
            .named_children(&mut cursor2)
            .filter(|c| c.kind() == "arguments_statement")
        {
            if let Some(attributes) = argument
                .named_children(&mut cursor3)
                .find(|c| c.kind() == "attributes")
            {
                if attributes
                    .named_children(&mut cursor4)
                    .filter_map(|c| c.utf8_text(parsed_file.contents.as_bytes()).ok())
                    .any(|c| c == "Output")
                {
                    continue;
                }
            }
            for property in argument
                .named_children(&mut cursor3)
                .filter_map(|c| c.child_by_field_name("name"))
                .filter(|c| c.kind() == "property_name")
            {
                let arg_name = property
                    .named_child(0)
                    .ok_or(anyhow!(code_loc!()))?
                    .utf8_text(parsed_file.contents.as_bytes())?
                    .to_string();
                argin_names.retain(|e| *e != arg_name);
                optional_arguments.insert(arg_name, ());
                let opt_arg_name = property
                    .named_child(1)
                    .ok_or(anyhow!(code_loc!()))?
                    .utf8_text(parsed_file.contents.as_bytes())?
                    .to_string();
                vargin_names.push(opt_arg_name);
            }
        }
        let vargin_count = optional_arguments.keys().count();
        vargin = vargin_count > 0;
        argin -= vargin_count;
    }
    let doc: String = node
        .named_children(&mut cursor)
        .skip_while(|n| n.kind() != "comment")
        .take(1)
        .flat_map(|n| n.utf8_text(parsed_file.contents.as_bytes()))
        .flat_map(|s| s.split('\n'))
        .map(|s| s.trim().to_string())
        .map(|s| s.strip_prefix('%').unwrap_or(s.as_str()).to_string())
        .join("\n");
    let function = FunctionSignature {
        name_range: name_range.into(),
        name,
        argin,
        argout,
        vargin,
        vargout,
        argout_names,
        argin_names,
        vargin_names,
        range: sig_range,
        documentation: doc,
    };
    Ok(function)
}
