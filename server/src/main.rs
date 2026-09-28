//! `gitlens-ls`: a language server that brings GitLens-style git features to
//! Zed. Zed extensions cannot draw UI, so everything here goes through
//! standard LSP features Zed already renders:
//!
//! - Code action "Show line history": runs `git log -L` over the selection,
//!   writes the result to a `.diff` file and opens it with `window/showDocument`.
//! - Code lens at the top of each file: last change and author count. Clicking
//!   it opens the last commit's diff for that file.

mod git;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lsp_server::{Connection, Message, Request, RequestId, Response};
use lsp_types::{
    request::{CodeActionRequest, CodeLensRequest, ExecuteCommand, Request as _, ShowDocument},
    CodeAction, CodeActionOrCommand, CodeActionParams, CodeActionProviderCapability, CodeLens,
    CodeLensOptions, CodeLensParams, Command, ExecuteCommandOptions, ExecuteCommandParams,
    Position, Range, ServerCapabilities, ShowDocumentParams, Url,
};
use serde_json::{json, Value};

use git::RepoFile;

const LINE_HISTORY: &str = "gitlens.lineHistory";
const SHOW_COMMIT: &str = "gitlens.showCommit";

fn main() -> Result<()> {
    let (connection, io_threads) = Connection::stdio();
    let capabilities = ServerCapabilities {
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        code_lens_provider: Some(CodeLensOptions {
            resolve_provider: Some(false),
        }),
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: vec![LINE_HISTORY.into(), SHOW_COMMIT.into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    connection.initialize(serde_json::to_value(capabilities)?)?;

    let mut server = Server {
        connection: &connection,
        next_request_id: 0,
    };
    for message in &connection.receiver {
        if let Message::Request(request) = message {
            if connection.handle_shutdown(&request)? {
                break;
            }
            server.handle(request)?;
        }
        // Notifications (didOpen, didChange, ...) and responses need no handling:
        // everything is read from git on demand.
    }
    // The writer thread only stops once every sender is dropped.
    drop(connection);
    io_threads.join()?;
    Ok(())
}

struct Server<'a> {
    connection: &'a Connection,
    next_request_id: i32,
}

impl Server<'_> {
    fn handle(&mut self, request: Request) -> Result<()> {
        let id = request.id.clone();
        let result = match request.method.as_str() {
            CodeActionRequest::METHOD => {
                code_actions(serde_json::from_value(request.params)?).map(|a| json!(a))
            }
            CodeLensRequest::METHOD => {
                code_lenses(serde_json::from_value(request.params)?).map(|l| json!(l))
            }
            ExecuteCommand::METHOD => self
                .execute_command(serde_json::from_value(request.params)?)
                .map(|()| Value::Null),
            _ => {
                let response = Response::new_err(
                    id,
                    lsp_server::ErrorCode::MethodNotFound as i32,
                    format!("unhandled method {}", request.method),
                );
                return Ok(self.connection.sender.send(response.into())?);
            }
        };
        let response = match result {
            Ok(value) => Response::new_ok(id, value),
            Err(error) => Response::new_err(
                id,
                lsp_server::ErrorCode::RequestFailed as i32,
                format!("{error:#}"),
            ),
        };
        Ok(self.connection.sender.send(response.into())?)
    }

    fn execute_command(&mut self, params: ExecuteCommandParams) -> Result<()> {
        let args = params.arguments;
        let uri: Url = serde_json::from_value(args.first().cloned().context("missing uri")?)?;
        let repo_file = RepoFile::locate(&file_path(&uri)?)?;
        let name = Path::new(&repo_file.relative)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        let (contents, output_name) = match params.command.as_str() {
            LINE_HISTORY => {
                let start = arg_u32(&args, 1)?;
                let end = arg_u32(&args, 2)?;
                let history = repo_file.line_history(start, end)?;
                (history, format!("{name}.L{start}-{end}.history.diff"))
            }
            SHOW_COMMIT => {
                let sha = args.get(1).and_then(Value::as_str).context("missing sha")?;
                let short = &sha[..sha.len().min(8)];
                (repo_file.show_commit(sha)?, format!("{name}.{short}.diff"))
            }
            other => anyhow::bail!("unknown command {other}"),
        };

        let path = write_output(&output_name, &contents)?;
        self.show_document(&path)
    }

    fn show_document(&mut self, path: &Path) -> Result<()> {
        let uri = Url::from_file_path(path)
            .map_err(|()| anyhow::anyhow!("invalid path {}", path.display()))?;
        let params = ShowDocumentParams {
            uri,
            external: Some(false),
            take_focus: Some(true),
            selection: None,
        };
        self.next_request_id += 1;
        let request = Request::new(
            RequestId::from(format!("gitlens-{}", self.next_request_id)),
            ShowDocument::METHOD.to_string(),
            params,
        );
        Ok(self.connection.sender.send(request.into())?)
    }
}

fn code_actions(params: CodeActionParams) -> Result<Vec<CodeActionOrCommand>> {
    let (start, end) = selected_lines(params.range);
    let title = if start == end {
        format!("Show line history (line {start})")
    } else {
        format!("Show line history (lines {start}-{end})")
    };
    let command = Command {
        title: title.clone(),
        command: LINE_HISTORY.into(),
        arguments: Some(vec![
            json!(params.text_document.uri),
            json!(start),
            json!(end),
        ]),
    };
    Ok(vec![CodeActionOrCommand::CodeAction(CodeAction {
        title,
        command: Some(command),
        ..Default::default()
    })])
}

fn code_lenses(params: CodeLensParams) -> Result<Vec<CodeLens>> {
    let uri = params.text_document.uri;
    let Ok(repo_file) = RepoFile::locate(&file_path(&uri)?) else {
        return Ok(Vec::new()); // Not in a git repository.
    };
    let Some(summary) = repo_file.summary()? else {
        return Ok(Vec::new());
    };
    let authors = match summary.authors {
        1 => "1 author".to_string(),
        n => format!("{n} authors"),
    };
    let top = Position::new(0, 0);
    Ok(vec![CodeLens {
        range: Range::new(top, top),
        command: Some(Command {
            title: format!("{}, {} · {authors}", summary.last_author, summary.last_when),
            command: SHOW_COMMIT.into(),
            arguments: Some(vec![json!(uri), json!(summary.last_sha)]),
        }),
        data: None,
    }])
}

/// Converts an LSP selection to 1-based, inclusive line numbers. A selection
/// that ends at column 0 of a later line does not include that line.
fn selected_lines(range: Range) -> (u32, u32) {
    let start = range.start.line;
    let mut end = range.end.line;
    if range.end.character == 0 && end > start {
        end -= 1;
    }
    (start + 1, end + 1)
}

fn file_path(uri: &Url) -> Result<PathBuf> {
    uri.to_file_path()
        .map_err(|()| anyhow::anyhow!("not a file uri: {uri}"))
}

fn arg_u32(args: &[Value], index: usize) -> Result<u32> {
    args.get(index)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .with_context(|| format!("argument {index} must be a line number"))
}

fn write_output(name: &str, contents: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("gitlens-ls");
    fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    fs::write(&path, contents)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_to_lines() {
        let range = |sl, sc, el, ec| Range::new(Position::new(sl, sc), Position::new(el, ec));
        assert_eq!(selected_lines(range(4, 3, 4, 3)), (5, 5));
        assert_eq!(selected_lines(range(4, 0, 9, 2)), (5, 10));
        assert_eq!(selected_lines(range(4, 0, 10, 0)), (5, 10));
        assert_eq!(selected_lines(range(4, 0, 4, 0)), (5, 5));
    }
}
