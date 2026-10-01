//! `x man`: roff man pages generated from the CLI grammar.
//!
//! Like completions, the pages come straight from the parsed [`crate::Cli`]
//! tree via `clap_mangen` — every subcommand becomes its own `x-<cmd>(1)`
//! page, so docs can never drift from the actual flags.

use clap::CommandFactory;
use clap_mangen::Man;
use x_core::error::Result;

use crate::format::Renderer;

/// `x man` arguments.
#[derive(Debug, clap::Args)]
pub struct ManArgs {
    /// Write one `x-<command>.1` roff file per subcommand into this
    /// directory instead of printing the top-level page to stdout.
    #[arg(long, value_name = "DIR")]
    pub dir: Option<String>,
}

/// Route a `x man` invocation.
pub fn dispatch(
    _context: &x_core::SystemContext,
    renderer: &mut Renderer,
    args: &ManArgs,
) -> Result<i32> {
    let cli = crate::Cli::command();

    match &args.dir {
        Some(dir) => {
            std::fs::create_dir_all(dir)
                .map_err(|e| x_core::Error::system(format!("cannot create {dir}: {e}")))?;
            write_page(&cli, dir)?;
            for sub in cli.get_subcommands() {
                write_subcommand_pages(sub, "x", dir)?;
            }
            renderer.line(format!("man pages written to {dir}"))?;
        }
        None => {
            let mut buffer = Vec::new();
            Man::new(cli.clone())
                .render(&mut buffer)
                .map_err(|e| x_core::Error::system(format!("cannot render man page: {e}")))?;
            renderer.raw(&buffer)?;
        }
    }
    Ok(0)
}

fn write_page(command: &clap::Command, dir: &str) -> Result<()> {
    let path = std::path::Path::new(dir).join(format!("{}.1", command.get_name()));
    let mut buffer = Vec::new();
    Man::new(command.clone())
        .render(&mut buffer)
        .map_err(|e| x_core::Error::system(format!("cannot render {}: {e}", path.display())))?;
    std::fs::write(&path, buffer)
        .map_err(|e| x_core::Error::system(format!("cannot write {}: {e}", path.display())))
}

/// Walk the subcommand tree depth first, mirroring clap's own naming:
/// `x-port.1`, `x-port-kill.1`, ...
fn write_subcommand_pages(command: &clap::Command, prefix: &str, dir: &str) -> Result<()> {
    let name = format!("{prefix}-{}", command.get_name());
    let mut nested = command.clone();
    nested.build();
    write_page(&nested, dir)?;
    for sub in nested.get_subcommands() {
        write_subcommand_pages(sub, &name, dir)?;
    }
    Ok(())
}
