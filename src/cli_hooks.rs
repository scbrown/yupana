//! `yupana hooks …` — install yupana's own hooks into Claude Code and Codex
//! (aegis-5s32or). See [`crate::hooks_install`] for the mechanism.

use anyhow::Result;
use clap::Subcommand;

use crate::hooks_install::{self as hi, Harness};

/// What `yupana hooks` does.
#[derive(Debug, Subcommand)]
pub enum HooksAction {
    /// Print yupana's hook bundle (schema `st.hook-bundle/1`): the one source of
    /// truth for which hooks yupana installs.
    Bundle,
    /// Install yupana's hooks. With shantytown present this registers the
    /// bundle, so st keeps it rendered into every role's Claude AND Codex
    /// settings; otherwise it writes the harness config directly.
    Install(Target),
    /// Remove yupana's hooks (only yupana's; other hooks are untouched).
    Uninstall(Target),
    /// Report, per harness, whether yupana's hooks are installed.
    Status(Target),
}

/// Where to install, when no shantytown registry is present.
#[derive(Debug, clap::Args)]
pub struct Target {
    /// Harness to write directly (no st). Repeatable; default both.
    #[arg(long, value_enum)]
    harness: Vec<Harness>,
    /// Write the project config (`./.claude`, `./.codex`) instead of the user's.
    #[arg(long)]
    project: bool,
    /// Write the harness config directly even when st is present.
    #[arg(long)]
    no_st: bool,
}

impl Target {
    fn harnesses(&self) -> Vec<Harness> {
        if self.harness.is_empty() {
            vec![Harness::Claude, Harness::Codex]
        } else {
            self.harness.clone()
        }
    }
}

/// Run one `yupana hooks` action. Returns the process exit code.
pub fn run(action: &HooksAction) -> Result<i32> {
    let bundle = hi::bundle();
    let target = match action {
        HooksAction::Bundle => {
            println!("{}", serde_json::to_string_pretty(&bundle)?);
            return Ok(0);
        }
        HooksAction::Install(t) | HooksAction::Uninstall(t) | HooksAction::Status(t) => t,
    };
    let via_st = !target.no_st && hi::st_available();
    match action {
        HooksAction::Install(_) | HooksAction::Uninstall(_) if via_st => {
            let register = matches!(action, HooksAction::Install(_));
            println!("{}", hi::st_register(register)?);
            println!(
                "yupana hooks {} through shantytown: st renders them into every role's \
                 claude and codex settings. Check with `st ops hooks check`.",
                if register {
                    "registered"
                } else {
                    "unregistered"
                }
            );
            Ok(0)
        }
        HooksAction::Status(_) if via_st => {
            let out = std::process::Command::new("st")
                .args(["ops", "hooks", "check", "--json"])
                .output()?;
            let report: serde_json::Value = serde_json::from_slice(&out.stdout)?;
            let mut ok = true;
            for h in target.harnesses() {
                let name = format!("{h:?}").to_lowercase();
                let items: Vec<_> = report["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|i| i["bundle"] == "yupana" && i["harness"] == name.as_str())
                    .collect();
                let count = |k: &str, v: &str| items.iter().filter(|i| i[k] == v).count();
                println!(
                    "{name}: {} hook item(s) via st; configured ok {}, live ok {}, firing ok {}, silent {}",
                    items.len(),
                    count("configured", "ok"),
                    count("live", "ok"),
                    count("firing", "ok"),
                    count("firing", "silent"),
                );
                ok &= !items.is_empty() && count("configured", "ok") == items.len();
            }
            Ok(i32::from(!ok))
        }
        _ => {
            let mut ok = true;
            for h in target.harnesses() {
                let path = hi::config_path(h, target.project);
                let mut cfg = hi::read_config(&path, h)?;
                let name = format!("{h:?}").to_lowercase();
                match action {
                    HooksAction::Install(_) => {
                        let n = hi::merge(&mut cfg, &bundle, h);
                        if n > 0 {
                            hi::write_config(&path, h, &cfg)?;
                        }
                        println!("{name}: added {n} hook(s) to {}", path.display());
                    }
                    HooksAction::Uninstall(_) => {
                        let n = hi::remove(&mut cfg, &bundle, h);
                        if n > 0 {
                            hi::write_config(&path, h, &cfg)?;
                        }
                        println!("{name}: removed {n} hook(s) from {}", path.display());
                    }
                    _ => {
                        let (have, want) = hi::present(&cfg, &bundle, h);
                        println!("{name}: {have}/{want} yupana hook(s) in {}", path.display());
                        ok &= have == want;
                    }
                }
            }
            Ok(i32::from(!ok))
        }
    }
}
