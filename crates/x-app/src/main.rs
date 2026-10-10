//! The `x` binary: composition root, nothing else.
//!
//! ```text
//! x <command> …   → x-cli   (one shot, scriptable, json capable)
//! x tui           → x-tui   (interactive, same context, same models)
//! ```
//!
//! Both frontends receive the *same* [`x_core::context::SystemContext`], which is
//! built here, in the one place allowed to know which platform it runs on. That
//! is why a port killed from the TUI shows up identically in `x port list`.

use std::process::ExitCode;

/// Argument that switches the binary into interactive mode.
const TUI_ARGUMENTS: [&str; 2] = ["tui", "ui"];

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().collect();
    let interactive = arguments
        .get(1)
        .is_some_and(|argument| TUI_ARGUMENTS.contains(&argument.as_str()));

    // The one place in the workspace that knows which platform it runs on.
    let context = match x_platform::create_context() {
        Ok(context) => context,
        Err(error) => return fail(if interactive { "x tui" } else { "x" }, &error),
    };
    // Destructive operations (kills, port reclaiming, service actions) leave
    // an audit trail from here on, in both frontends.
    let context = x_platform::audit::attach(context);

    if interactive {
        // The composition root knows the platform, so it wires the net-top
        // sampler in here; x-tui itself only sees the trait object. Opening
        // the capture device is best-effort: without root the page degrades
        // to a hint instead of failing the TUI.
        let net_top: std::sync::Arc<dyn x_core::net_top::NetTopSampler> =
            std::sync::Arc::new(x_platform::PlatformNetSampler::new(None));
        return match x_tui::run(context, Some(net_top)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => fail("x tui", &error),
        };
    }

    ExitCode::from(u8::try_from(x_cli::from_args_with(&context)).unwrap_or(1))
}

/// Report a startup failure and turn it into an exit code.
fn fail(label: &str, error: &x_core::Error) -> ExitCode {
    eprintln!("{label}: {}", error.message());
    ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(1))
}
