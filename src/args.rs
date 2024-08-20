/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

pub use clap::Parser;

static LONG_ABOUT: &str = "
matlab-lsp is a very simple LSP for MATLAB(R)

It only offers some very basic functionality.";

#[derive(Debug, Parser)]
#[command(author, version, about = LONG_ABOUT)]
pub struct Arguments {
    // A UNIX-like path. Files inside this folder will also be analyzed.
    #[arg(global = true, long = "path", short = 'p', env = "MLSP_PATH")]
    pub path: Option<String>,
}
